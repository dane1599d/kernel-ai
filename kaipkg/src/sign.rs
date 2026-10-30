//! Writing a TUF 1.0.37 repository in standard JSON (ADR 0011 §3, ADR 0008 §7):
//! the packer's on the host (`cargo xtask pkgrepo`) and `pkg`'s `local` one in
//! the OS ([`crate::local`]), so the two never drift.
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
//! Keys are Ed25519 (`ed25519-compact`). Every file is canonical JSON
//! (`kaituf::canonical`, the verifier's own), and each signature is over the
//! canonical bytes of its `signed` object.

use crate::generation::Time;
use crate::{hex, sha256};
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use ed25519_compact::{KeyPair, Seed};
use serde_json::{Map, Value, json};

pub const SPEC_VERSION: &str = "1.0.37";
const DAY: u64 = 86_400;
/// The delegated role every package sits under, and what it may sign for.
pub const PACKAGES: &str = "packages";
const PACKAGE_PATHS: &str = "*.kpkg";

/// How long each role's metadata stays valid, in days.
#[derive(Clone, Copy, Debug)]
pub struct Periods {
    pub root: u64,
    pub targets: u64,
    pub snapshot: u64,
    pub timestamp: u64,
}

/// ADR 0008 §7's periods.
pub const ADR_0008: Periods = Periods {
    root: 365,
    targets: 365,
    snapshot: 30,
    timestamp: 7,
};

/// One role's Ed25519 key.
#[derive(Clone)]
pub struct Key {
    pair: KeyPair,
}

impl Key {
    pub fn from_seed(seed: [u8; Seed::BYTES]) -> Self {
        Key {
            pair: KeyPair::from_seed(Seed::new(seed)),
        }
    }

    /// The key as TUF writes it in `keys`.
    pub fn public(&self) -> Value {
        json!({
            "keytype": "ed25519",
            "scheme": "ed25519",
            "keyval": { "public": hex_bytes(self.pair.pk.as_ref()) },
        })
    }

    /// The SHA-256 of the canonical public key, as tuf-on-ci and python-tuf name keys.
    pub fn id(&self) -> String {
        let public = canonical(&self.public()).expect("a key is plain JSON");
        hex(&sha256(&public))
    }

    fn sign(&self, signed: &Value) -> Result<Value, String> {
        let bytes = canonical(signed)?;
        let sig = self.pair.sk.sign(bytes, None);
        Ok(json!({ "keyid": self.id(), "sig": hex_bytes(sig.as_ref()) }))
    }
}

/// The keys of one repository, by role.
pub struct Keys<'a> {
    pub root: &'a Key,
    pub targets: &'a Key,
    pub snapshot: &'a Key,
    pub timestamp: &'a Key,
    pub packages: &'a Key,
}

impl<'a> Keys<'a> {
    /// One key for every role, as a device's own repository has.
    pub fn one(key: &'a Key) -> Self {
        Keys {
            root: key,
            targets: key,
            snapshot: key,
            timestamp: key,
            packages: key,
        }
    }
}

/// One package as a target: its path `<name>-<version>.kpkg`, its bytes and its
/// `custom.kai` index fields.
pub struct Target {
    pub path: String,
    pub bytes: Vec<u8>,
    pub kai: Value,
}

/// The repository for `targets`, every role at version 1, signed by `keys`,
/// expiring `periods` after `now`: its files, by path from its top.
pub fn build(
    targets: &[Target],
    keys: &Keys<'_>,
    now: Time,
    periods: Periods,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let expires = |days: u64| Time(now.0 + days * DAY).format();
    let mut files = Vec::new();
    let mut listed = Map::new();
    let mut names = BTreeSet::new();
    for target in targets {
        if !names.insert(target.path.clone()) {
            return Err(format!("{} is in the repository twice", target.path));
        }
        let sha = hex(&sha256(&target.bytes));
        listed.insert(
            target.path.clone(),
            json!({
                "length": target.bytes.len(),
                "hashes": { "sha256": sha },
                "custom": { "kai": target.kai },
            }),
        );
        files.push((
            format!("targets/{sha}.{}", target.path),
            target.bytes.clone(),
        ));
    }
    let root = json!({
        "_type": "root",
        "spec_version": SPEC_VERSION,
        "version": 1,
        "expires": expires(periods.root),
        "consistent_snapshot": true,
        "keys": key_map(&[keys.root, keys.targets, keys.snapshot, keys.timestamp]),
        "roles": {
            "root": role(keys.root),
            "targets": role(keys.targets),
            "snapshot": role(keys.snapshot),
            "timestamp": role(keys.timestamp),
        },
    });
    let top = json!({
        "_type": "targets",
        "spec_version": SPEC_VERSION,
        "version": 1,
        "expires": expires(periods.targets),
        "targets": {},
        "delegations": {
            "keys": key_map(&[keys.packages]),
            "roles": [{
                "name": PACKAGES,
                "keyids": [keys.packages.id()],
                "threshold": 1,
                "terminating": true,
                "paths": [PACKAGE_PATHS],
            }],
        },
    });
    let delegated = json!({
        "_type": "targets",
        "spec_version": SPEC_VERSION,
        "version": 1,
        "expires": expires(periods.targets),
        "targets": listed,
    });
    let snapshot = json!({
        "_type": "snapshot",
        "spec_version": SPEC_VERSION,
        "version": 1,
        "expires": expires(periods.snapshot),
        "meta": {
            "targets.json": { "version": 1 },
            "packages.json": { "version": 1 },
        },
    });
    let snapshot = envelope(&snapshot, keys.snapshot)?;
    let timestamp = json!({
        "_type": "timestamp",
        "spec_version": SPEC_VERSION,
        "version": 1,
        "expires": expires(periods.timestamp),
        "meta": {
            "snapshot.json": {
                "version": 1,
                "length": snapshot.len(),
                "hashes": { "sha256": hex(&sha256(&snapshot)) },
            },
        },
    });
    files.extend([
        ("metadata/1.root.json".into(), envelope(&root, keys.root)?),
        (
            "metadata/1.targets.json".into(),
            envelope(&top, keys.targets)?,
        ),
        (
            "metadata/1.packages.json".into(),
            envelope(&delegated, keys.packages)?,
        ),
        ("metadata/1.snapshot.json".into(), snapshot),
        (
            "metadata/timestamp.json".into(),
            envelope(&timestamp, keys.timestamp)?,
        ),
    ]);
    Ok(files)
}

/// `keys` as a role's `keys` map: each key's id to its public key.
pub fn key_map(keys: &[&Key]) -> Value {
    Value::Object(keys.iter().map(|k| (k.id(), k.public())).collect())
}

/// A role signed by `key` alone.
pub fn role(key: &Key) -> Value {
    json!({ "keyids": [key.id()], "threshold": 1 })
}

/// `signed` with its one signature, as canonical JSON.
pub fn envelope(signed: &Value, key: &Key) -> Result<Vec<u8>, String> {
    canonical(&json!({ "signed": signed, "signatures": [key.sign(signed)?] }))
}

/// OLPC canonical JSON, as securesystemslib writes it for TUF: kaituf's own,
/// so what is signed is what the verifier checks.
pub fn canonical(value: &Value) -> Result<Vec<u8>, String> {
    kaituf::canonical::canonical(value).map_err(|e| e.to_string())
}

/// `bytes` as lower-case hex.
pub fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 8032 §7.1, test 1: the key and the signature of the empty message.
    #[test]
    fn keys_sign_as_rfc_8032_says() {
        let seed = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
        let seed: Vec<u8> = (0..64)
            .step_by(2)
            .map(|i| u8::from_str_radix(&seed[i..i + 2], 16).unwrap())
            .collect();
        let key = Key::from_seed(seed.try_into().unwrap());
        assert_eq!(
            key.public()["keyval"]["public"],
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
        );
        assert_eq!(
            hex_bytes(key.pair.sk.sign(b"", None).as_ref()),
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065\
             224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
        );
        assert_eq!(key.id(), hex(&sha256(&canonical(&key.public()).unwrap())));
    }
}
