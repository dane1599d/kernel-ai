//! The client workflow (TUF 1.0.37 §5.3-§5.7, as python-tuf's ngclient runs it)
//! over a [`Remote`] that fetches metadata files and a [`Cache`] that keeps the
//! verified ones between refreshes.
//!
//! A refresh loads root versions until the repository has no next one, then
//! timestamp, snapshot and the top-level targets. Each role is tried from the
//! cache first, as an intermediate that guards against rollback; the cached
//! snapshot and targets are used as they are when they are still final, and
//! everything else is downloaded, verified and only then cached. Targets are
//! found by a preorder depth-first search of the delegations ([`Updater::target`]);
//! fetching and checking the artifact is the caller's ([`Updater::target_url`],
//! [`check_target`]).

use crate::meta::{TargetFile, check_hashes};
use crate::trusted::{ROOT, TrustedSet};
use crate::{Error, quote};
use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

/// Size limits, as python-tuf's defaults.
pub const ROOT_MAX: u64 = 512_000;
pub const TIMESTAMP_MAX: u64 = 16_384;
pub const SNAPSHOT_MAX: u64 = 2_000_000;
pub const TARGETS_MAX: u64 = 5_000_000;
/// How many root versions one refresh may move through.
pub const MAX_ROOT_ROTATIONS: u64 = 256;
/// How many targets roles one search may visit.
pub const MAX_DELEGATIONS: usize = 32;

/// The repository's metadata store.
pub trait Remote {
    /// The file `name` (such as `2.root.json` or `timestamp.json`) of at most
    /// `max` bytes; None when the repository has none (HTTP 404 or 403). A
    /// longer file is [`Error::TooLarge`].
    fn fetch(&mut self, name: &str, max: u64) -> Result<Option<Vec<u8>>, Error>;
}

/// Where verified metadata is kept between refreshes, by role name, as the
/// bytes the repository served.
pub trait Cache {
    fn load(&mut self, role: &str) -> Option<Vec<u8>>;
    fn save(&mut self, role: &str, bytes: &[u8]) -> Result<(), Error>;
}

pub struct Updater<'a> {
    remote: &'a mut dyn Remote,
    cache: &'a mut dyn Cache,
    trusted: TrustedSet,
}

/// `remote`'s file `name`, at most `max` bytes, or why not.
fn fetch(remote: &mut dyn Remote, name: &str, max: u64) -> Result<Vec<u8>, Error> {
    match remote.fetch(name, max)? {
        Some(bytes) if bytes.len() as u64 <= max => Ok(bytes),
        Some(_) => Err(Error::TooLarge(name.into())),
        None => Err(Error::NotFound(name.into())),
    }
}

impl<'a> Updater<'a> {
    /// From the cache's root, which must verify; `now` is the reference time,
    /// seconds since 1970 UTC.
    pub fn new(
        remote: &'a mut dyn Remote,
        cache: &'a mut dyn Cache,
        now: u64,
    ) -> Result<Self, Error> {
        let root = cache
            .load("root")
            .ok_or(Error::NotFound("no trusted root".into()))?;
        let trusted = TrustedSet::new(&root, now)?;
        Ok(Updater {
            remote,
            cache,
            trusted,
        })
    }

    pub fn trusted(&self) -> &TrustedSet {
        &self.trusted
    }

    /// The top-level roles, current and verified.
    pub fn refresh(&mut self) -> Result<(), Error> {
        self.load_root()?;
        self.load_timestamp()?;
        self.load_snapshot()?;
        self.load_targets("targets", ROOT)
    }

    fn load_root(&mut self) -> Result<(), Error> {
        let first = self.trusted.root().version + 1;
        for version in first..first + MAX_ROOT_ROTATIONS {
            let name = alloc::format!("{version}.root.json");
            let Some(bytes) = self.remote.fetch(&name, ROOT_MAX)? else {
                break;
            };
            if bytes.len() as u64 > ROOT_MAX {
                return Err(Error::TooLarge(name));
            }
            self.trusted.update_root(&bytes)?;
            self.cache.save("root", &bytes)?;
        }
        Ok(())
    }

    fn load_timestamp(&mut self) -> Result<(), Error> {
        if let Some(bytes) = self.cache.load("timestamp") {
            // An intermediate: whatever it fails, the download below is checked
            // against what it held.
            let _ = self.trusted.update_timestamp(&bytes);
        }
        let bytes = fetch(self.remote, "timestamp.json", TIMESTAMP_MAX)?;
        if self.trusted.update_timestamp(&bytes)? {
            self.cache.save("timestamp", &bytes)?;
        }
        Ok(())
    }

    fn load_snapshot(&mut self) -> Result<(), Error> {
        if let Some(bytes) = self.cache.load("snapshot")
            && self.trusted.update_snapshot(&bytes, true).is_ok()
        {
            return Ok(());
        }
        let timestamp = self
            .trusted
            .timestamp()
            .ok_or(Error::Malformed("no timestamp".into()))?;
        let named = &timestamp.signed.snapshot;
        let name = match self.trusted.root().signed.consistent_snapshot {
            true => alloc::format!("{}.snapshot.json", named.version),
            false => "snapshot.json".into(),
        };
        let bytes = fetch(self.remote, &name, named.length.unwrap_or(SNAPSHOT_MAX))?;
        self.trusted.update_snapshot(&bytes, false)?;
        self.cache.save("snapshot", &bytes)
    }

    /// Targets role `role` as `delegator` delegates it: already loaded, or the
    /// cached copy if it is current, or downloaded.
    fn load_targets(&mut self, role: &str, delegator: &str) -> Result<(), Error> {
        if self.trusted.trusted_through(role, delegator)? {
            return Ok(());
        }
        if let Some(bytes) = self.cache.load(role)
            && self.trusted.update_targets(&bytes, role, delegator).is_ok()
        {
            return Ok(());
        }
        let snapshot = self
            .trusted
            .snapshot()
            .ok_or(Error::Malformed("no snapshot".into()))?;
        let file = alloc::format!("{role}.json");
        let named = snapshot.signed.meta.get(&file);
        let named = named
            .ok_or_else(|| Error::NotFound(alloc::format!("{role} is not in the snapshot")))?;
        let name = match self.trusted.root().signed.consistent_snapshot {
            true => alloc::format!("{}.{}.json", named.version, quote(role)),
            false => alloc::format!("{}.json", quote(role)),
        };
        let bytes = fetch(self.remote, &name, named.length.unwrap_or(TARGETS_MAX))?;
        self.trusted.update_targets(&bytes, role, delegator)?;
        self.cache.save(role, &bytes)
    }

    /// The paths targets role `role` lists, loaded as `delegator` delegates it,
    /// for a client that takes every target of one role, as `pkg` takes the
    /// image's packages. Look each up with [`Updater::target`], which says what
    /// the first role to name it holds. Call [`Updater::refresh`] first.
    pub fn listed(&mut self, role: &str, delegator: &str) -> Result<Vec<String>, Error> {
        self.load_targets(role, delegator)?;
        let targets = self
            .trusted
            .targets(role)
            .ok_or_else(|| Error::Malformed(alloc::format!("{role} did not load")))?;
        Ok(targets.signed.targets.keys().cloned().collect())
    }

    /// The artifact `path` as the first role to name it in a preorder depth-first
    /// search of the delegations says it is; None if no trusted role names it.
    /// Call [`Updater::refresh`] first.
    pub fn target(&mut self, path: &str) -> Result<Option<TargetFile>, Error> {
        let mut stack: Vec<(String, String)> = alloc::vec![("targets".into(), ROOT.into())];
        let mut visited: BTreeSet<(String, String)> = BTreeSet::new();
        let mut left = MAX_DELEGATIONS;
        while left > 0 {
            let Some((role, delegator)) = stack.pop() else {
                break;
            };
            if visited.contains(&(role.clone(), delegator.clone())) {
                continue;
            }
            self.load_targets(&role, &delegator)?;
            let Some(targets) = self.trusted.targets(&role) else {
                return Err(Error::Malformed(alloc::format!("{role} did not load")));
            };
            if let Some(found) = targets.signed.targets.get(path) {
                return Ok(Some(found.clone()));
            }
            let mut children = Vec::new();
            for child in targets.signed.delegations.iter().flat_map(|d| &d.roles) {
                if child.matches(path) {
                    children.push((child.name.clone(), role.clone()));
                    if child.terminating {
                        stack.clear();
                        break;
                    }
                }
            }
            visited.insert((role, delegator));
            left -= 1;
            stack.extend(children.into_iter().rev());
        }
        Ok(None)
    }

    /// Where `target` is under the repository's targets URL: with consistent
    /// snapshots, its file name prefixed by its first hash.
    pub fn target_url(&self, target: &TargetFile) -> String {
        match (
            self.trusted.root().signed.consistent_snapshot,
            target.hashes.first(),
        ) {
            (true, Some((_, digest))) => match target.path.rsplit_once('/') {
                Some((dir, file)) => alloc::format!("{dir}/{}.{file}", crate::hex(digest)),
                None => alloc::format!("{}.{}", crate::hex(digest), target.path),
            },
            _ => target.path.clone(),
        }
    }
}

/// Whether `bytes` are `target`: its length, and every hash it names.
pub fn check_target(target: &TargetFile, bytes: &[u8]) -> Result<(), Error> {
    if bytes.len() as u64 != target.length {
        return Err(Error::Hash(alloc::format!(
            "{} is {} bytes, not {}",
            target.path,
            bytes.len(),
            target.length
        )));
    }
    check_hashes(&target.hashes, bytes, &target.path)
}
