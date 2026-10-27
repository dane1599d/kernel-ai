//! The trusted set (TUF 1.0.37 §5, as python-tuf's `TrustedMetadataSet` keeps
//! it): what this refresh has verified so far, against one reference time.
//!
//! Each update verifies before it keeps. Some are kept even when they fail a
//! final check, as the spec's client does, so a local copy that is no longer
//! current still guards against rollback: an expired timestamp or snapshot,
//! or a snapshot whose version is not the timestamp's. Those failures are still
//! returned, and the next role refuses to load on top of them.
//!
//! Delegated roles are kept by name with the delegators they were verified
//! through: reaching one through another delegator verifies it again against
//! that delegator's keys (tuf-conformance's
//! `test_cached_role_is_verified_against_current_delegator`).

use crate::keys::Key;
use crate::meta::{Meta, Role, Root, Snapshot, Targets, Timestamp};
use crate::{Error, malformed};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// The pseudo-delegator of the top-level targets role.
pub const ROOT: &str = "root";

pub struct TrustedSet {
    /// The reference time: seconds since 1970 UTC.
    now: u64,
    root: Meta<Root>,
    timestamp: Option<Meta<Timestamp>>,
    snapshot: Option<Meta<Snapshot>>,
    /// By role name: the metadata and the delegators it was verified through.
    targets: BTreeMap<String, (Meta<Targets>, Vec<String>)>,
}

/// Whether `role`'s threshold of its `keys` signed `meta`. A role that falls
/// short while some of its keys are of a type this client does not check is
/// refused as [`Error::UnsupportedKey`].
pub fn verify<T>(
    meta: &Meta<T>,
    keys: &BTreeMap<String, Key>,
    role: &Role,
    name: &str,
) -> Result<(), Error> {
    let mut valid = 0;
    let mut unsupported = None;
    for id in &role.keyids {
        let Some(key) = keys.get(id) else {
            continue;
        };
        if let Key::Unsupported(kind) = key {
            unsupported = Some(kind);
        }
        if meta
            .sigs
            .iter()
            .any(|(k, sig)| k == id && key.verifies(&meta.canonical, sig))
        {
            valid += 1;
        }
    }
    match (valid >= role.threshold, unsupported) {
        (true, _) => Ok(()),
        (false, Some(kind)) => Err(Error::UnsupportedKey(alloc::format!(
            "{name} needs {} signatures, and some of its keys are {kind}",
            role.threshold
        ))),
        (false, None) => Err(Error::Threshold(alloc::format!(
            "{name} has {valid} valid signatures of the {} it needs",
            role.threshold
        ))),
    }
}

impl TrustedSet {
    /// Trusts `root`, which must be signed by its own root role. Its expiry is
    /// checked once the root update is over ([`TrustedSet::update_timestamp`]).
    pub fn new(root: &[u8], now: u64) -> Result<Self, Error> {
        let root = Root::parse(root)?;
        verify(&root, &root.signed.keys, root.signed.role("root"), "root")?;
        Ok(TrustedSet {
            now,
            root,
            timestamp: None,
            snapshot: None,
            targets: BTreeMap::new(),
        })
    }

    pub fn root(&self) -> &Meta<Root> {
        &self.root
    }

    pub fn timestamp(&self) -> Option<&Meta<Timestamp>> {
        self.timestamp.as_ref()
    }

    pub fn snapshot(&self) -> Option<&Meta<Snapshot>> {
        self.snapshot.as_ref()
    }

    /// The targets role `name` verified so far, top-level or delegated.
    pub fn targets(&self, name: &str) -> Option<&Meta<Targets>> {
        self.targets.get(name).map(|(meta, _)| meta)
    }

    /// The next root: signed by the trusted root's root role and by its own,
    /// with the next version.
    pub fn update_root(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if self.timestamp.is_some() {
            return malformed("root cannot change once timestamp is loaded");
        }
        let next = Root::parse(bytes)?;
        verify(
            &next,
            &self.root.signed.keys,
            self.root.signed.role("root"),
            "root",
        )?;
        if next.version != self.root.version + 1 {
            return Err(Error::Version(alloc::format!(
                "root {} is not {}",
                next.version,
                self.root.version + 1
            )));
        }
        verify(&next, &next.signed.keys, next.signed.role("root"), "root")?;
        self.root = next;
        Ok(())
    }

    /// A timestamp, unless the trusted root has expired. Ok(false) means it has
    /// the trusted one's version, so the trusted one stays.
    pub fn update_timestamp(&mut self, bytes: &[u8]) -> Result<bool, Error> {
        self.unexpired(self.root.expires, "root")?;
        let next = Timestamp::parse(bytes)?;
        verify(
            &next,
            &self.root.signed.keys,
            self.root.signed.role("timestamp"),
            "timestamp",
        )?;
        if let Some(old) = &self.timestamp {
            if next.version < old.version {
                return Err(Error::Version(alloc::format!(
                    "timestamp {} is older than {}",
                    next.version,
                    old.version
                )));
            }
            if next.version == old.version {
                return Ok(false);
            }
            if next.signed.snapshot.version < old.signed.snapshot.version {
                return Err(Error::Version("timestamp names an older snapshot".into()));
            }
        }
        let expires = next.expires;
        self.timestamp = Some(next);
        self.unexpired(expires, "timestamp")?;
        Ok(true)
    }

    /// A snapshot, checked against the timestamp's hashes unless it comes from
    /// the local cache, and against every targets role the trusted snapshot
    /// names: none may go missing or back.
    pub fn update_snapshot(&mut self, bytes: &[u8], cached: bool) -> Result<(), Error> {
        let timestamp = self
            .timestamp
            .as_ref()
            .ok_or(Error::Malformed("snapshot before timestamp".into()))?;
        self.unexpired(timestamp.expires, "timestamp")?;
        let named = timestamp.signed.snapshot.clone();
        if !cached {
            named.check(bytes, "snapshot")?;
        }
        let next = Snapshot::parse(bytes)?;
        verify(
            &next,
            &self.root.signed.keys,
            self.root.signed.role("snapshot"),
            "snapshot",
        )?;
        if let Some(old) = &self.snapshot {
            for (file, meta) in &old.signed.meta {
                match next.signed.meta.get(file) {
                    None => {
                        return Err(Error::Version(alloc::format!(
                            "snapshot no longer names {file}"
                        )));
                    }
                    Some(new) if new.version < meta.version => {
                        return Err(Error::Version(alloc::format!(
                            "snapshot names an older {file}"
                        )));
                    }
                    Some(_) => {}
                }
            }
        }
        self.snapshot = Some(next);
        self.final_snapshot()
    }

    /// The trusted snapshot must be the timestamp's version and unexpired.
    fn final_snapshot(&self) -> Result<(), Error> {
        let (Some(snapshot), Some(timestamp)) = (&self.snapshot, &self.timestamp) else {
            return malformed("no snapshot");
        };
        if snapshot.version != timestamp.signed.snapshot.version {
            return Err(Error::Version(alloc::format!(
                "snapshot {} is not the timestamp's {}",
                snapshot.version,
                timestamp.signed.snapshot.version
            )));
        }
        self.unexpired(snapshot.expires, "snapshot")
    }

    /// The keys and role `delegator` gives `name`.
    fn delegation(
        &self,
        name: &str,
        delegator: &str,
    ) -> Result<(&BTreeMap<String, Key>, &Role), Error> {
        if delegator == ROOT {
            return match name {
                "targets" => Ok((&self.root.signed.keys, self.root.signed.role("targets"))),
                _ => malformed("root delegates only targets"),
            };
        }
        let parent = self
            .targets(delegator)
            .ok_or(Error::Malformed(alloc::format!(
                "{delegator} is not loaded"
            )))?;
        let delegations = parent.signed.delegations.as_ref();
        let role = delegations.and_then(|d| d.roles.iter().find(|r| r.name == name));
        match (delegations, role) {
            (Some(d), Some(role)) => Ok((&d.keys, &role.role)),
            _ => malformed(alloc::format!("{delegator} does not delegate to {name}")),
        }
    }

    /// Targets role `name` as `delegator` delegates it: in the snapshot, with
    /// its hashes where the snapshot names them, signed by the delegation's
    /// keys, the snapshot's version, unexpired.
    pub fn update_targets(
        &mut self,
        bytes: &[u8],
        name: &str,
        delegator: &str,
    ) -> Result<(), Error> {
        self.final_snapshot()?;
        let file = alloc::format!("{name}.json");
        let named = self
            .snapshot
            .as_ref()
            .and_then(|s| s.signed.meta.get(&file));
        let named = named
            .ok_or_else(|| Error::NotFound(alloc::format!("{name} is not in the snapshot")))?;
        named.check(bytes, name)?;
        let version = named.version;
        let next = Targets::parse(bytes)?;
        let (keys, role) = self.delegation(name, delegator)?;
        verify(&next, keys, role, name)?;
        if next.version != version {
            return Err(Error::Version(alloc::format!(
                "{name} {} is not the snapshot's {version}",
                next.version
            )));
        }
        self.unexpired(next.expires, name)?;
        self.targets
            .insert(name.into(), (next, alloc::vec![delegator.to_string()]));
        Ok(())
    }

    /// Whether loaded role `name` holds as `delegator` delegates it: verified
    /// against that delegation now if it was loaded through another.
    pub fn trusted_through(&mut self, name: &str, delegator: &str) -> Result<bool, Error> {
        let Some((meta, via)) = self.targets.get(name) else {
            return Ok(false);
        };
        if via.iter().any(|d| d == delegator) {
            return Ok(true);
        }
        let (keys, role) = self.delegation(name, delegator)?;
        verify(meta, keys, role, name)?;
        if let Some((_, via)) = self.targets.get_mut(name) {
            via.push(delegator.into());
        }
        Ok(true)
    }

    fn unexpired(&self, expires: u64, name: &str) -> Result<(), Error> {
        match expires > self.now {
            true => Ok(()),
            false => Err(Error::Expired(name.into())),
        }
    }
}
