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
//! signature is over the canonical bytes of its `signed` object.

use super::Package;
use ed25519_compact::{KeyPair, Seed};
use kaipkg::generation::Time;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

pub const SPEC_VERSION: &str = "1.0.37";
const DAY: u64 = 86_400;
/// How long each role's metadata stays valid: ADR 0008 §7's periods.
const ROOT_DAYS: u64 = 365;
const TARGETS_DAYS: u64 = 365;
const SNAPSHOT_DAYS: u64 = 30;
const TIMESTAMP_DAYS: u64 = 7;
/// The delegated role every package sits under, and what it may sign for.
pub const PACKAGES: &str = "packages";
const PACKAGE_PATHS: &str = "*.kpkg";

/// One role's Ed25519 key.
pub struct Key {
    pair: KeyPair,
}

impl Key {
    /// A fresh key from the OS's randomness.
    pub fn generate() -> Result<Self, String> {
        let mut seed = [0u8; Seed::BYTES];
        getrandom::getrandom(&mut seed).map_err(|e| format!("no randomness for a key: {e}"))?;
        Ok(Self::from_seed(seed))
    }

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
            "keyval": { "public": hex(self.pair.pk.as_ref()) },
        })
    }

    /// The SHA-256 of the canonical public key, as tuf-on-ci and python-tuf name keys.
    pub fn id(&self) -> String {
        crate::sha256_hex(&canonical(&self.public()).expect("a key is plain JSON"))
    }

    fn sign(&self, signed: &Value) -> Result<Value, String> {
        let sig = self.pair.sk.sign(canonical(signed)?, None);
        Ok(json!({ "keyid": self.id(), "sig": hex(sig.as_ref()) }))
    }
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
            root: Key::generate()?,
            targets: Key::generate()?,
            snapshot: Key::generate()?,
            timestamp: Key::generate()?,
            packages: Key::generate()?,
        })
    }
}

/// A repository's files, by path from its top.
pub struct Repository {
    pub files: Vec<(String, Vec<u8>)>,
}

/// The repository for `packages`, every role at version 1, signed by `keys`,
/// with expiry counted from `now`.
pub fn build(packages: &[Package], keys: &Keys, now: Time) -> Result<Repository, String> {
    let expires = |days: u64| Time(now.0 + days * DAY).format();
    let mut files = Vec::new();
    let mut targets = Map::new();
    let mut names = BTreeSet::new();
    for package in packages {
        let target = package.target();
        if !names.insert(target.clone()) {
            return Err(format!("{target} is in the repository twice"));
        }
        let sha = crate::sha256_hex(&package.bytes);
        targets.insert(
            target.clone(),
            json!({
                "length": package.bytes.len(),
                "hashes": { "sha256": sha },
                "custom": { "kai": package.index() },
            }),
        );
        files.push((format!("targets/{sha}.{target}"), package.bytes.clone()));
    }
    let root = json!({
        "_type": "root",
        "spec_version": SPEC_VERSION,
        "version": 1,
        "expires": expires(ROOT_DAYS),
        "consistent_snapshot": true,
        "keys": key_map(&[&keys.root, &keys.targets, &keys.snapshot, &keys.timestamp]),
        "roles": {
            "root": role(&keys.root),
            "targets": role(&keys.targets),
            "snapshot": role(&keys.snapshot),
            "timestamp": role(&keys.timestamp),
        },
    });
    let top = json!({
        "_type": "targets",
        "spec_version": SPEC_VERSION,
        "version": 1,
        "expires": expires(TARGETS_DAYS),
        "targets": {},
        "delegations": {
            "keys": key_map(&[&keys.packages]),
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
        "expires": expires(TARGETS_DAYS),
        "targets": targets,
    });
    let snapshot = json!({
        "_type": "snapshot",
        "spec_version": SPEC_VERSION,
        "version": 1,
        "expires": expires(SNAPSHOT_DAYS),
        "meta": {
            "targets.json": { "version": 1 },
            "packages.json": { "version": 1 },
        },
    });
    let snapshot = envelope(&snapshot, &keys.snapshot)?;
    let timestamp = json!({
        "_type": "timestamp",
        "spec_version": SPEC_VERSION,
        "version": 1,
        "expires": expires(TIMESTAMP_DAYS),
        "meta": {
            "snapshot.json": {
                "version": 1,
                "length": snapshot.len(),
                "hashes": { "sha256": crate::sha256_hex(&snapshot) },
            },
        },
    });
    files.extend([
        ("metadata/1.root.json".into(), envelope(&root, &keys.root)?),
        (
            "metadata/1.targets.json".into(),
            envelope(&top, &keys.targets)?,
        ),
        (
            "metadata/1.packages.json".into(),
            envelope(&delegated, &keys.packages)?,
        ),
        ("metadata/1.snapshot.json".into(), snapshot),
        (
            "metadata/timestamp.json".into(),
            envelope(&timestamp, &keys.timestamp)?,
        ),
    ]);
    Ok(Repository { files })
}

fn key_map(keys: &[&Key]) -> Value {
    Value::Object(keys.iter().map(|k| (k.id(), k.public())).collect())
}

fn role(key: &Key) -> Value {
    json!({ "keyids": [key.id()], "threshold": 1 })
}

/// `signed` with its one signature, as canonical JSON.
fn envelope(signed: &Value, key: &Key) -> Result<Vec<u8>, String> {
    canonical(&json!({ "signed": signed, "signatures": [key.sign(signed)?] }))
}

/// OLPC canonical JSON, as securesystemslib writes it for TUF: no whitespace,
/// object keys in byte order, integers only, and in strings only `"` and `\`
/// escaped. A control character or a fraction has no canonical form and is
/// refused.
pub fn canonical(value: &Value) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    write_canonical(value, &mut out)?;
    Ok(out)
}

fn write_canonical(value: &Value, out: &mut Vec<u8>) -> Result<(), String> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Number(n) if n.is_i64() || n.is_u64() => {
            out.extend_from_slice(n.to_string().as_bytes())
        }
        Value::Number(n) => return Err(format!("{n} is not an integer")),
        Value::String(s) => {
            if s.chars().any(char::is_control) {
                return Err(format!("{s:?} holds a control character"));
            }
            out.push(b'"');
            for b in s.bytes() {
                if matches!(b, b'"' | b'\\') {
                    out.push(b'\\');
                }
                out.push(b);
            }
            out.push(b'"');
        }
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(item, out)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            out.push(b'{');
            for (i, (key, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(&Value::String(key.clone()), out)?;
                out.push(b':');
                write_canonical(item, out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_sorts_keys_and_escapes_only_quotes_and_backslashes() {
        let value = json!({ "b": [1, true, null], "a": "x\"y\\z/é", "B": -2 });
        assert_eq!(
            String::from_utf8(canonical(&value).unwrap()).unwrap(),
            r#"{"B":-2,"a":"x\"y\\z/é","b":[1,true,null]}"#
        );
        assert!(canonical(&json!({ "a": 1.5 })).is_err());
        assert!(canonical(&json!(["a\nb"])).is_err());
    }

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
            hex(key.pair.sk.sign(b"", None).as_ref()),
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065\
             224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"
        );
        assert_eq!(
            key.id(),
            crate::sha256_hex(&canonical(&key.public()).unwrap())
        );
    }
}
