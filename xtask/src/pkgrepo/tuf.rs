//! A TUF 1.0.37 repository in standard JSON (ADR 0011 §3, ADR 0008 §7), signed
//! by a throwaway test root (P4 plan, section 4, item 3).
//!
//! The top-level roles are root, targets, snapshot and timestamp; targets
//! delegates every `*.kpkg` to the role `packages`, which lists each package as
//! a target `<name>-<version>.kpkg` with its length, SHA-256 and `custom.kai`
//! index fields. Snapshots are consistent, so the layout is:
//!
//! ```text
//! metadata/1.root.json
//! metadata/1.targets.json
//! metadata/1.packages.json
//! metadata/1.snapshot.json
//! metadata/timestamp.json
//! targets/<sha256>.notes-1.0.0.kpkg
//! ```
//!
//! Keys are Ed25519 (`ed25519-compact`), one per role, made fresh for each
//! build and never written down. Every file is canonical JSON, and each
//! signature is over the canonical bytes of its `signed` object. The writing is
//! `kaipkg::sign`'s, which `pkg` signs its `local` repository with too.

use super::Package;
use ed25519_compact::Seed;
use kaipkg::generation::Time;
use kaipkg::sign::{self, ADR_0008, Target};
pub use kaipkg::sign::{Key, SPEC_VERSION, envelope, key_map, role};

/// A fresh key from the OS's randomness.
pub fn generate_key() -> Result<Key, String> {
    let mut seed = [0u8; Seed::BYTES];
    getrandom::getrandom(&mut seed).map_err(|e| format!("no randomness for a key: {e}"))?;
    Ok(Key::from_seed(seed))
}

/// The keys of one repository.
pub struct Keys {
    pub root: Key,
    pub targets: Key,
    pub snapshot: Key,
    pub timestamp: Key,
    pub packages: Key,
}

impl Keys {
    /// A throwaway test root: five fresh keys.
    pub fn generate() -> Result<Self, String> {
        Ok(Keys {
            root: generate_key()?,
            targets: generate_key()?,
            snapshot: generate_key()?,
            timestamp: generate_key()?,
            packages: generate_key()?,
        })
    }

    fn roles(&self) -> sign::Keys<'_> {
        sign::Keys {
            root: &self.root,
            targets: &self.targets,
            snapshot: &self.snapshot,
            timestamp: &self.timestamp,
            packages: &self.packages,
        }
    }
}

/// A repository's files, by path from its top.
pub struct Repository {
    pub files: Vec<(String, Vec<u8>)>,
}

/// The repository for `packages`, every role at version 1, signed by `keys`,
/// with ADR 0008 §7's expiry counted from `now`.
pub fn build(packages: &[Package], keys: &Keys, now: Time) -> Result<Repository, String> {
    let targets: Vec<Target> = packages
        .iter()
        .map(|package| Target {
            path: package.target(),
            bytes: package.bytes.clone(),
            kai: package.index(),
        })
        .collect();
    let files = sign::build(&targets, &keys.roles(), now, ADR_0008)?;
    Ok(Repository { files })
}
