//! Writing a TUF 1.0.37 repository in standard JSON (ADR 0011 §3, ADR 0008 §7):
//! the packer's on the host (`cargo xtask pkgrepo`) and `pkg`'s `local` one in
//! the OS ([`crate::local`]), so the two never drift.
//!
//! The top-level roles are root, targets, snapshot and timestamp; targets
//! delegates every `*.kpkg` to the role `packages`, which lists each package as
//! a target `<name>-<version>.kpkg` with its length, SHA-256 and `custom.kai`
//! index fields. With a models key, targets also delegates every `*.gguf` and
//! `*.chunks` to the role `models` (ADR 0008 §6), which lists each model as
//! `<id>.gguf` ([`crate::models`]); a model's bytes are never in the
//! repository. A model a download fetches also has its `kai-chunks 1` list
//! beside it, the target `<sha256 of the GGUF>.chunks` with its bytes in the
//! repository (P5 task 6), so each chunk is checked before it is written. A
//! device's own repository has no models key, so it signs no models: only the
//! image's role says which a user may import. Snapshots are consistent, so the
//! layout is:
//!
//! ```text
//! metadata/1.root.json
//! metadata/1.targets.json
//! metadata/1.packages.json
//! metadata/1.models.json
//! metadata/1.snapshot.json
//! metadata/timestamp.json
//! targets/<sha256>.notes-1.0.0.kpkg
//! targets/<sha256 of the list>.<sha256 of the GGUF>.chunks
//! ```
//!
//! Keys are Ed25519 (`ed25519-compact`). Every file is canonical JSON
//! (`kaituf::canonical`, the verifier's own), and each signature is over the
//! canonical bytes of its `signed` object.

use crate::generation::Time;
use crate::{Sha, hex, sha256};
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use ed25519_compact::{KeyPair, Seed};
use serde_json::{Map, Value, json};

pub const SPEC_VERSION: &str = "1.0.37";
const DAY: u64 = 86_400;
/// The delegated role every package sits under, and what it may sign for.
pub const PACKAGES: &str = "packages";
const PACKAGE_PATHS: &[&str] = &["*.kpkg"];
/// The delegated role every model sits under, and what it may sign for.
pub const MODELS: &str = crate::models::ROLE;
const MODEL_PATHS: &[&str] = &["*.gguf", "*.chunks"];

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
    /// The `models` role's, or none for a repository that signs no models.
    pub models: Option<&'a Key>,
}

impl<'a> Keys<'a> {
    /// One key for every role, as a device's own repository has; it has no
    /// `models` role.
    pub fn one(key: &'a Key) -> Self {
        Keys {
            root: key,
            targets: key,
            snapshot: key,
            timestamp: key,
            packages: key,
            models: None,
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

/// One model as a target of the `models` role: its path `<id>.gguf`, the
/// length and SHA-256 of the GGUF, and its `custom.kai` fields. The weights are
/// not in the repository, so it has no bytes.
pub struct Model {
    pub path: String,
    pub length: u64,
    pub sha256: Sha,
    pub kai: Value,
    /// Its `kai-chunks 1` list, signed beside it as the target
    /// [`crate::models::chunks_path`] with these bytes in the repository, for a
    /// download to check each chunk against; `None` for a model only imported.
    /// The caller checks that the list names this model's length and SHA-256.
    pub chunks: Option<Vec<u8>>,
}

/// The repository for `targets`, with no models: [`build_with_models`].
pub fn build(
    targets: &[Target],
    keys: &Keys<'_>,
    now: Time,
    periods: Periods,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    build_with_models(targets, &[], keys, now, periods)
}

/// The repository for `targets` and `models`, every role at version 1, signed
/// by `keys`, expiring `periods` after `now`: its files, by path from its top.
/// The `models` role is there when `keys` has its key, empty or not.
pub fn build_with_models(
    targets: &[Target],
    models: &[Model],
    keys: &Keys<'_>,
    now: Time,
    periods: Periods,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    if keys.models.is_none() && !models.is_empty() {
        return Err("models are signed only with a models key".into());
    }
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
    let model_targets = listed_models(models, &mut names, &mut files)?;
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
    let mut delegated_keys = vec![keys.packages];
    let mut roles = vec![delegation(PACKAGES, keys.packages, PACKAGE_PATHS)];
    let mut meta = Map::new();
    meta.insert("targets.json".into(), json!({ "version": 1 }));
    meta.insert("packages.json".into(), json!({ "version": 1 }));
    if let Some(key) = keys.models {
        delegated_keys.push(key);
        roles.push(delegation(MODELS, key, MODEL_PATHS));
        meta.insert(format!("{MODELS}.json"), json!({ "version": 1 }));
    }
    let top = json!({
        "_type": "targets",
        "spec_version": SPEC_VERSION,
        "version": 1,
        "expires": expires(periods.targets),
        "targets": {},
        "delegations": { "keys": key_map(&delegated_keys), "roles": roles },
    });
    let delegated = |targets: Map<String, Value>| {
        json!({
            "_type": "targets",
            "spec_version": SPEC_VERSION,
            "version": 1,
            "expires": expires(periods.targets),
            "targets": targets,
        })
    };
    let snapshot = json!({
        "_type": "snapshot",
        "spec_version": SPEC_VERSION,
        "version": 1,
        "expires": expires(periods.snapshot),
        "meta": meta,
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
            envelope(&delegated(listed), keys.packages)?,
        ),
        ("metadata/1.snapshot.json".into(), snapshot),
        (
            "metadata/timestamp.json".into(),
            envelope(&timestamp, keys.timestamp)?,
        ),
    ]);
    if let Some(key) = keys.models {
        files.push((
            format!("metadata/1.{MODELS}.json"),
            envelope(&delegated(model_targets), key)?,
        ));
    }
    Ok(files)
}

/// `models` as the `models` role's targets, each path new to `names`, with
/// each chunk list's bytes added to `files`.
fn listed_models(
    models: &[Model],
    names: &mut BTreeSet<String>,
    files: &mut Vec<(String, Vec<u8>)>,
) -> Result<Map<String, Value>, String> {
    let mut listed = Map::new();
    for model in models {
        if !model.path.ends_with(crate::models::EXTENSION) {
            return Err(format!("{} is not a model's path", model.path));
        }
        if !names.insert(model.path.clone()) {
            return Err(format!("{} is in the repository twice", model.path));
        }
        listed.insert(
            model.path.clone(),
            json!({
                "length": model.length,
                "hashes": { "sha256": hex(&model.sha256) },
                "custom": { "kai": model.kai },
            }),
        );
        let Some(list) = &model.chunks else {
            continue;
        };
        let path = crate::models::chunks_path(&model.sha256);
        if !names.insert(path.clone()) {
            return Err(format!("{path} is in the repository twice"));
        }
        let sha = hex(&sha256(list));
        listed.insert(
            path.clone(),
            json!({ "length": list.len(), "hashes": { "sha256": sha } }),
        );
        files.push((format!("targets/{sha}.{path}"), list.clone()));
    }
    Ok(listed)
}

/// A terminating delegation of `paths` to the role `name`, signed by `key`.
fn delegation(name: &str, key: &Key, paths: &[&str]) -> Value {
    json!({
        "name": name,
        "keyids": [key.id()],
        "threshold": 1,
        "terminating": true,
        "paths": paths,
    })
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

    fn model() -> Model {
        Model {
            path: "tiny.gguf".into(),
            length: 4096,
            sha256: [7; 32],
            kai: json!({ "name": "Tiny" }),
            chunks: None,
        }
    }

    fn paths(files: &[(String, Vec<u8>)]) -> Vec<&str> {
        files.iter().map(|(p, _)| p.as_str()).collect()
    }

    #[test]
    fn a_devices_own_repository_signs_no_models() {
        let key = Key::from_seed([1; Seed::BYTES]);
        let local = build(&[], &Keys::one(&key), Time(0), ADR_0008).unwrap();
        assert!(paths(&local).iter().all(|p| !p.contains(MODELS)));
        let with = build_with_models(&[], &[model()], &Keys::one(&key), Time(0), ADR_0008);
        assert!(with.is_err(), "no models key, no models");
    }

    #[test]
    fn a_models_key_signs_each_model_without_its_bytes() {
        let key = Key::from_seed([1; Seed::BYTES]);
        let models_key = Key::from_seed([2; Seed::BYTES]);
        let keys = Keys {
            models: Some(&models_key),
            ..Keys::one(&key)
        };
        let files = build_with_models(&[], &[model()], &keys, Time(0), ADR_0008).unwrap();
        assert!(paths(&files).iter().all(|p| !p.starts_with("targets/")));
        let (_, role) = files
            .iter()
            .find(|(p, _)| p == "metadata/1.models.json")
            .unwrap();
        let role: Value = serde_json::from_slice(role).unwrap();
        let target = &role["signed"]["targets"]["tiny.gguf"];
        assert_eq!(target["length"], 4096);
        assert_eq!(target["hashes"]["sha256"], hex(&[7; 32]));
        assert_eq!(role["signatures"][0]["keyid"], models_key.id());
        let not_a_model = Model {
            path: "tiny.kpkg".into(),
            ..model()
        };
        assert!(build_with_models(&[], &[not_a_model], &keys, Time(0), ADR_0008).is_err());
    }

    #[test]
    fn a_models_chunk_list_is_signed_beside_it_with_its_bytes() {
        let key = Key::from_seed([1; Seed::BYTES]);
        let models_key = Key::from_seed([2; Seed::BYTES]);
        let keys = Keys {
            models: Some(&models_key),
            ..Keys::one(&key)
        };
        let list = b"kai-chunks 1\n".to_vec();
        let with = Model {
            chunks: Some(list.clone()),
            ..model()
        };
        let files = build_with_models(&[], &[with], &keys, Time(0), ADR_0008).unwrap();
        let file = |path: &str| files.iter().find(|(p, _)| p == path).map(|(_, b)| b);
        let role: Value = serde_json::from_slice(file("metadata/1.models.json").unwrap()).unwrap();
        let name = format!("{}.chunks", hex(&[7; 32]));
        let target = &role["signed"]["targets"][&name];
        let sha = hex(&sha256(&list));
        assert_eq!(target["length"], list.len());
        assert_eq!(target["hashes"]["sha256"], sha);
        assert!(target.get("custom").is_none());
        assert_eq!(file(&format!("targets/{sha}.{name}")), Some(&list));
        // The model itself still has no bytes.
        assert!(role["signed"]["targets"]["tiny.gguf"].is_object());
        assert_eq!(
            paths(&files)
                .iter()
                .filter(|p| p.starts_with("targets/"))
                .count(),
            1
        );
        // The role is delegated both kinds of path.
        let top: Value = serde_json::from_slice(file("metadata/1.targets.json").unwrap()).unwrap();
        let delegated = &top["signed"]["delegations"]["roles"][1];
        assert_eq!(delegated["name"], MODELS);
        assert_eq!(delegated["paths"], json!(["*.gguf", "*.chunks"]));
        // Two models with one GGUF would sign one list twice.
        let twin = || Model {
            chunks: Some(list.clone()),
            ..model()
        };
        let second = Model {
            path: "twin.gguf".into(),
            ..twin()
        };
        let both = build_with_models(&[], &[twin(), second], &keys, Time(0), ADR_0008);
        assert!(both.unwrap_err().contains("twice"));
    }
}
