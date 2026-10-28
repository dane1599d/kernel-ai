//! The kernel's side of the COMP_* calls for one component, as plain data: its
//! versions, how often each was fetched, its uploads and its [`Ledger`]. The kernel
//! keeps one per component; tests and xtask use the same code as a fake kernel.

use crate::artifact;
use crate::ledger::{Event, Ledger, Phase};
use crate::uploads::{self, Meta, Outcome, Uploads};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use kabi::Error;
use kabi::live::{
    Info, MAX_ARTIFACT, MAX_LABEL, MAX_PACKAGE_TOTAL, MAX_VERSIONS, Reason, Restore,
    is_package_label, is_upload_label, kind, package_label, phase, put_label, valid_label,
};
use sha2::{Digest, Sha256};

/// A version's bytes: a boot module, or an upload on the heap. Cloning shares them.
#[derive(Clone, Debug)]
pub enum Bytes {
    Static(&'static [u8]),
    Shared(Arc<Vec<u8>>),
}

impl Bytes {
    pub fn shared(bytes: Vec<u8>) -> Self {
        Self::Shared(Arc::new(bytes))
    }

    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Static(bytes) => bytes,
            Self::Shared(bytes) => bytes,
        }
    }
}

/// Where a version came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Boot,
    /// Built by the edit loop from version `base`, its source in git `commit`.
    Upload {
        seq: u64,
        base: String,
        commit: String,
    },
    /// From an installed package, added by `pkg` (P4 task 1): its artifact's full
    /// SHA-256, which the label's 12 hex digits begin.
    Package {
        sha: [u8; 32],
    },
}

#[derive(Clone, Debug)]
pub struct Version {
    pub label: String,
    pub bytes: Bytes,
    pub fetches: u64,
    pub origin: Origin,
    /// From the manifest, read once when the version was added; empty when it has
    /// none (the host then refuses to load it).
    pub world: String,
    /// SHA-256 of the source bundle, from the manifest.
    pub source: String,
    /// The source's file paths, sorted; an upload must keep them.
    pub files: Vec<String>,
}

impl Version {
    pub fn is_upload(&self) -> bool {
        matches!(self.origin, Origin::Upload { .. })
    }

    pub fn commit(&self) -> Option<&str> {
        match &self.origin {
            Origin::Upload { commit, .. } => Some(commit),
            Origin::Boot | Origin::Package { .. } => None,
        }
    }
}

/// An artifact from an installed package whose SHA-256 was hashed here and
/// matched the digest given: the only way into [`Component::add_package`]. The
/// kernel makes one before it takes the component lock.
#[derive(Debug)]
pub struct PackageArtifact {
    bytes: Vec<u8>,
    sha: [u8; 32],
}

impl PackageArtifact {
    /// `bytes`, if they are at most [`MAX_ARTIFACT`] (TooBig) and hash to `sha`
    /// (Invalid).
    pub fn check(bytes: Vec<u8>, sha: &[u8; 32]) -> Result<Self, Error> {
        if bytes.len() > MAX_ARTIFACT {
            return Err(Error::TooBig);
        }
        let own: [u8; 32] = Sha256::digest(&bytes).into();
        if own != *sha {
            return Err(Error::Invalid);
        }
        Ok(Self { bytes, sha: own })
    }
}

#[derive(Clone, Debug)]
pub struct Component {
    pub name: String,
    pub(crate) versions: Vec<Version>,
    pub ledger: Ledger,
    pub uploads: Uploads,
}

impl Component {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            versions: Vec::new(),
            ledger: Ledger::new(),
            uploads: Uploads::new(),
        }
    }

    /// Adds a boot version. Upload labels (`e` and digits) are kept for uploads,
    /// and package labels (`p-` and 12 hex digits) for packages.
    pub fn add_version(&mut self, label: &str, bytes: Bytes) -> Result<(), &'static str> {
        if !valid_label(label) {
            return Err("bad version label");
        }
        if is_upload_label(label) {
            return Err("e and a number is an upload label");
        }
        if is_package_label(label) {
            return Err("p- and 12 hex digits is a package label");
        }
        if bytes.as_slice().len() > MAX_ARTIFACT {
            return Err("artifact over 1 MiB");
        }
        if self.versions.len() == MAX_VERSIONS {
            return Err("too many versions");
        }
        if self.version(label).is_some() {
            return Err("version listed twice");
        }
        let (world, source, files) = match artifact::parse(bytes.as_slice()) {
            Ok(parsed) => (
                parsed.manifest.world,
                parsed.manifest.source,
                uploads::source_paths(parsed.source).unwrap_or_default(),
            ),
            Err(_) => (String::new(), String::new(), Vec::new()),
        };
        self.versions.push(Version {
            label: label.to_string(),
            bytes,
            fetches: 0,
            origin: Origin::Boot,
            world,
            source,
            files,
        });
        Ok(())
    }

    /// Adds the version an installed package holds (P4 task 1), labelled from
    /// its checked SHA-256. It only adds: a label already there is Busy, so a
    /// running or known-good version is never replaced. A full store is
    /// NoMemory, and so is one more than [`MAX_PACKAGE_TOTAL`] bytes of package
    /// versions with `used_elsewhere`, the other components' ([`Self::package_bytes`]):
    /// package versions are never evicted, so this bounds the kernel's heap. Its
    /// label.
    pub fn add_package(
        &mut self,
        artifact: PackageArtifact,
        used_elsewhere: usize,
    ) -> Result<String, Error> {
        let PackageArtifact { bytes, sha } = artifact;
        let label = package_label(&sha);
        if self.version(&label).is_some() {
            return Err(Error::Busy);
        }
        let total = used_elsewhere
            .saturating_add(self.package_bytes())
            .saturating_add(bytes.len());
        if self.versions.len() == MAX_VERSIONS || total > MAX_PACKAGE_TOTAL {
            return Err(Error::NoMemory);
        }
        let (world, source, files) = match artifact::parse(&bytes) {
            Ok(parsed) => (
                parsed.manifest.world,
                parsed.manifest.source,
                uploads::source_paths(parsed.source).unwrap_or_default(),
            ),
            Err(_) => (String::new(), String::new(), Vec::new()),
        };
        self.versions.push(Version {
            label: label.clone(),
            bytes: Bytes::shared(bytes),
            fetches: 0,
            origin: Origin::Package { sha },
            world,
            source,
            files,
        });
        Ok(label)
    }

    pub fn labels(&self) -> impl Iterator<Item = &str> {
        self.versions.iter().map(|v| v.label.as_str())
    }

    pub fn versions(&self) -> &[Version] {
        &self.versions
    }

    pub fn version(&self, label: &str) -> Option<&Version> {
        self.versions.iter().find(|v| v.label == label)
    }

    /// Version `label`'s bytes.
    pub fn bytes(&self, label: &str) -> Result<&Bytes, Error> {
        if !valid_label(label) {
            return Err(Error::Invalid);
        }
        self.version(label).map(|v| &v.bytes).ok_or(Error::NotFound)
    }

    /// Counts a fetch of `label` by `pid`, which may then report on it.
    pub fn record_fetch(&mut self, pid: u64, label: &str) {
        if let Some(version) = self.versions.iter_mut().find(|v| v.label == label) {
            version.fetches += 1;
            self.ledger.fetched(pid, label);
        }
    }

    /// Version `label` for process `pid`, when it fits in `cap` bytes; its length
    /// either way. Only a fetch that fits counts, and lets `pid` report on it.
    pub fn fetch(&mut self, pid: u64, label: &str, cap: usize) -> Result<(&[u8], bool), Error> {
        let len = self.bytes(label)?.as_slice().len();
        let fits = cap >= len;
        if fits {
            self.record_fetch(pid, label);
        }
        Ok((self.bytes(label)?.as_slice(), fits))
    }

    pub fn fetches(&self, label: &str) -> u64 {
        self.version(label).map_or(0, |v| v.fetches)
    }

    /// Applies a host's event. A version must still be stored to be started or
    /// swapped to, so an evicted upload can never become active.
    pub fn event(&mut self, pid: u64, event: &Event) -> Result<Option<String>, Error> {
        let starts = matches!(event.kind, kind::RESTORED | kind::BEGIN);
        if starts && valid_label(&event.label) && self.version(&event.label).is_none() {
            return Err(Error::NotFound);
        }
        let line = self.ledger.apply(pid, event)?;
        let outcome = match event.kind {
            kind::COMMIT => Some(Outcome::Commit),
            kind::ROLLBACK => Reason::from_raw(event.reason).map(Outcome::Rollback),
            _ => None,
        };
        if let Some(outcome) = outcome {
            let version = self.version(&event.label);
            let source = version.map_or("", |v| v.source.as_str()).to_string();
            let commit = version.and_then(Version::commit).unwrap_or("").to_string();
            let generation = self.ledger.generation;
            self.uploads
                .outbox
                .push(&event.label, outcome, generation, source, commit);
        }
        Ok(line)
    }

    /// The ledger as [`Restore`] reports it.
    pub fn restore(&self) -> Restore {
        let mut restore = Restore::empty();
        let ledger = &self.ledger;
        restore.phase = phase_of(ledger);
        restore.schema = ledger.schema;
        restore.generation = ledger.generation;
        restore.checkpoint_len = ledger.checkpoint.len() as u64;
        restore.flags = if ledger.disabled {
            Restore::DISABLED
        } else {
            0
        };
        restore.active_len = put_label(&mut restore.active, ledger.active().unwrap_or(""));
        restore.candidate_len = put_label(&mut restore.candidate, ledger.candidate().unwrap_or(""));
        let name = self.name.as_bytes();
        let len = name.len().min(kabi::MAX_NAME);
        restore.name[..len].copy_from_slice(&name[..len]);
        restore.name_len = len as u64;
        restore
    }

    /// The component as COMP_READ's INFO reports it.
    pub fn info(&self) -> Info {
        let mut info = Info::empty();
        let ledger = &self.ledger;
        info.phase = phase_of(ledger);
        info.generation = ledger.generation;
        info.flags = if ledger.disabled { Info::DISABLED } else { 0 };
        info.uploads = self.versions.iter().filter(|v| v.is_upload()).count() as u64;
        info.active_len = put_label(&mut info.active, ledger.active().unwrap_or(""));
        info.candidate_len = put_label(&mut info.candidate, ledger.candidate().unwrap_or(""));
        info.next_len = put_label(&mut info.next, &self.next_label());
        info
    }

    pub fn checkpoint(&self) -> &[u8] {
        &self.ledger.checkpoint
    }

    /// The label the next upload must carry.
    pub fn next_label(&self) -> String {
        self.uploads.next_label()
    }

    /// Bytes held by this component's uploads.
    /// Bytes of this component's package versions, which count against
    /// [`MAX_PACKAGE_TOTAL`] with every other component's.
    pub fn package_bytes(&self) -> usize {
        self.versions
            .iter()
            .filter(|v| matches!(v.origin, Origin::Package { .. }))
            .map(|v| v.bytes.as_slice().len())
            .sum()
    }

    pub fn upload_bytes(&self) -> usize {
        self.versions
            .iter()
            .filter(|v| v.is_upload())
            .map(|v| v.bytes.as_slice().len())
            .sum()
    }

    /// Stores an upload whose manifest `meta` was read from `bytes` (by
    /// [`uploads::meta`]) before taking the lock. `used_elsewhere` is what other
    /// components' uploads hold. On any refusal the store is unchanged; refusals by
    /// the rules, an unreadable artifact among them, count as rejected.
    pub fn put(
        &mut self,
        meta: &Result<Meta, Error>,
        bytes: Arc<Vec<u8>>,
        base: &str,
        commit: &str,
        used_elsewhere: usize,
    ) -> Result<String, Error> {
        let label = self.next_label();
        let active = self.ledger.active().and_then(|l| self.version(l));
        let checked = match meta {
            Ok(meta) => {
                let from = valid_label(base).then(|| self.version(base)).flatten();
                uploads::check(meta, &self.name, &label, active, from, commit).map(|()| meta)
            }
            Err(error) => Err(*error),
        };
        let meta = match checked {
            Ok(meta) => meta,
            Err(error) => {
                if error == Error::Invalid {
                    self.uploads.rejected += 1;
                }
                return Err(error);
            }
        };
        let victims = uploads::victims(self, bytes.len(), used_elsewhere)?;
        if self.versions.len() - victims.len() >= MAX_VERSIONS {
            return Err(Error::Busy);
        }
        self.versions.retain(|v| !victims.contains(&v.label));
        self.uploads.evicted += victims.len() as u64;
        let seq = self.uploads.take_seq();
        debug_assert!(label.len() <= MAX_LABEL);
        self.versions.push(Version {
            label: label.clone(),
            bytes: Bytes::Shared(bytes),
            fetches: 0,
            origin: Origin::Upload {
                seq,
                base: base.to_string(),
                commit: commit.to_string(),
            },
            world: meta.world.clone(),
            source: meta.source.clone(),
            files: meta.files.clone(),
        });
        self.uploads.accepted += 1;
        Ok(label)
    }
}

fn phase_of(ledger: &Ledger) -> u64 {
    match ledger.phase {
        Phase::Empty => phase::EMPTY,
        Phase::Steady { .. } => phase::STEADY,
        Phase::Swapping { .. } => phase::SWAPPING,
    }
}

#[cfg(test)]
mod tests;
