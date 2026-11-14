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
//!
//! Building and signing are apart ([`unsigned`], [`timestamp`],
//! [`signed_envelope`]), so a release's roles can be written out, signed
//! elsewhere with keys this code never holds, and put together after (P4 task
//! 18's key files, `cargo xtask release-repo`). [`build_with_models`] is the
//! same steps with keys in hand.

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
pub const DAY: u64 = 86_400;
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

/// One role's Ed25519 public key: what metadata names, and all of a key that
/// signs elsewhere, as a founder's key file, is here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicKey(pub [u8; 32]);

impl PublicKey {
    /// The key as TUF writes it in `keys`.
    pub fn public(&self) -> Value {
        json!({
            "keytype": "ed25519",
            "scheme": "ed25519",
            "keyval": { "public": hex_bytes(&self.0) },
        })
    }

    /// The SHA-256 of the canonical public key, as tuf-on-ci and python-tuf name keys.
    pub fn id(&self) -> String {
        let public = canonical(&self.public()).expect("a key is plain JSON");
        hex(&sha256(&public))
    }
}

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

    pub fn public_key(&self) -> PublicKey {
        let mut raw = [0u8; 32];
        raw.copy_from_slice(self.pair.pk.as_ref());
        PublicKey(raw)
    }

    /// The key as TUF writes it in `keys`.
    pub fn public(&self) -> Value {
        self.public_key().public()
    }

    /// The SHA-256 of the canonical public key, as tuf-on-ci and python-tuf name keys.
    pub fn id(&self) -> String {
        self.public_key().id()
    }

    /// The Ed25519 signature of `message`, as `openssl pkeyutl -sign -rawin`
    /// makes it with the same key: Ed25519 signs deterministically.
    pub fn sign_bytes(&self, message: &[u8]) -> [u8; 64] {
        let mut raw = [0u8; 64];
        raw.copy_from_slice(self.pair.sk.sign(message, None).as_ref());
        raw
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

    /// Each role's public key.
    pub fn roles(&self) -> Roles {
        Roles {
            root: self.root.public_key(),
            targets: self.targets.public_key(),
            snapshot: self.snapshot.public_key(),
            timestamp: self.timestamp.public_key(),
            packages: self.packages.public_key(),
            models: self.models.map(Key::public_key),
        }
    }
}

/// The public keys of one repository, by role: what its metadata names.
#[derive(Clone, Copy, Debug)]
pub struct Roles {
    pub root: PublicKey,
    pub targets: PublicKey,
    pub snapshot: PublicKey,
    pub timestamp: PublicKey,
    pub packages: PublicKey,
    /// The `models` role's, or none for a repository that signs no models.
    pub models: Option<PublicKey>,
}

/// One package as a target: its path `<name>-<version>.kpkg`, its bytes and its
/// `custom.kai` index fields.
pub struct Target {
    pub path: String,
    pub bytes: Vec<u8>,
    pub kai: Value,
}

/// `target`'s entry in its role's `targets`, exactly as [`unsigned`] writes
/// it: its length, its SHA-256 and its `custom.kai`, and nothing else.
pub fn target_entry(target: &Target) -> Value {
    json!({
        "length": target.bytes.len(),
        "hashes": { "sha256": hex(&sha256(&target.bytes)) },
        "custom": { "kai": target.kai },
    })
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
    let roles = unsigned(targets, models, &keys.roles(), now, periods, 1)?;
    let snapshot = envelope(&roles.snapshot, keys.snapshot)?;
    let timestamp = timestamp(&snapshot, 1, now, periods);
    let mut files = roles.files;
    files.extend([
        (path("root", 1), envelope(&roles.root, keys.root)?),
        (path("targets", 1), envelope(&roles.targets, keys.targets)?),
        (path(PACKAGES, 1), envelope(&roles.packages, keys.packages)?),
        (path("snapshot", 1), snapshot),
        (TIMESTAMP_PATH.into(), envelope(&timestamp, keys.timestamp)?),
    ]);
    if let (Some(signed), Some(key)) = (&roles.models, keys.models) {
        files.push((path(MODELS, 1), envelope(signed, key)?));
    }
    Ok(files)
}

/// Where a role's metadata at `version` sits, consistent snapshots' name:
/// `metadata/<version>.<role>.json`. The timestamp's is [`TIMESTAMP_PATH`].
pub fn path(role: &str, version: u64) -> String {
    format!("metadata/{version}.{role}.json")
}

/// Where the timestamp sits: the one name a client knows before any other.
pub const TIMESTAMP_PATH: &str = "metadata/timestamp.json";

/// A repository's roles before they are signed: each one's `signed` object,
/// and the target files beside them. The timestamp is not here, as it holds
/// the SHA-256 of the snapshot as signed ([`timestamp`]).
pub struct Unsigned {
    pub root: Value,
    pub targets: Value,
    pub packages: Value,
    /// The `models` role's, when the roles have its key.
    pub models: Option<Value>,
    pub snapshot: Value,
    /// Each target's bytes, by its consistent-snapshot path under `targets/`.
    pub files: Vec<(String, Vec<u8>)>,
}

/// The roles of the repository for `targets` and `models`, named by `roles`'
/// keys and expiring `periods` after `now`, unsigned: root at version 1, the
/// others at `version`, which a release takes from its `release_seq` so that
/// it only rises.
pub fn unsigned(
    targets: &[Target],
    models: &[Model],
    keys: &Roles,
    now: Time,
    periods: Periods,
    version: u64,
) -> Result<Unsigned, String> {
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
        listed.insert(target.path.clone(), target_entry(target));
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
        "keys": key_map(&[&keys.root, &keys.targets, &keys.snapshot, &keys.timestamp]),
        "roles": {
            "root": role(&keys.root),
            "targets": role(&keys.targets),
            "snapshot": role(&keys.snapshot),
            "timestamp": role(&keys.timestamp),
        },
    });
    let mut delegated_keys = vec![&keys.packages];
    let mut roles = vec![delegation(PACKAGES, &keys.packages, PACKAGE_PATHS)];
    let mut meta = Map::new();
    meta.insert("targets.json".into(), json!({ "version": version }));
    meta.insert("packages.json".into(), json!({ "version": version }));
    if let Some(key) = &keys.models {
        delegated_keys.push(key);
        roles.push(delegation(MODELS, key, MODEL_PATHS));
        meta.insert(format!("{MODELS}.json"), json!({ "version": version }));
    }
    let top = json!({
        "_type": "targets",
        "spec_version": SPEC_VERSION,
        "version": version,
        "expires": expires(periods.targets),
        "targets": {},
        "delegations": { "keys": key_map(&delegated_keys), "roles": roles },
    });
    let delegated = |targets: Map<String, Value>| {
        json!({
            "_type": "targets",
            "spec_version": SPEC_VERSION,
            "version": version,
            "expires": expires(periods.targets),
            "targets": targets,
        })
    };
    let snapshot = json!({
        "_type": "snapshot",
        "spec_version": SPEC_VERSION,
        "version": version,
        "expires": expires(periods.snapshot),
        "meta": meta,
    });
    Ok(Unsigned {
        root,
        targets: top,
        packages: delegated(listed),
        models: keys.models.map(|_| delegated(model_targets)),
        snapshot,
        files,
    })
}

/// The timestamp role at `version` for `snapshot`, the snapshot role's file
/// as signed, of the same version, expiring `periods` after `now`.
pub fn timestamp(snapshot: &[u8], version: u64, now: Time, periods: Periods) -> Value {
    json!({
        "_type": "timestamp",
        "spec_version": SPEC_VERSION,
        "version": version,
        "expires": Time(now.0 + periods.timestamp * DAY).format(),
        "meta": {
            "snapshot.json": {
                "version": version,
                "length": snapshot.len(),
                "hashes": { "sha256": hex(&sha256(snapshot)) },
            },
        },
    })
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
fn delegation(name: &str, key: &PublicKey, paths: &[&str]) -> Value {
    json!({
        "name": name,
        "keyids": [key.id()],
        "threshold": 1,
        "terminating": true,
        "paths": paths,
    })
}

/// `keys` as a role's `keys` map: each key's id to its public key. One key
/// named twice, as one key for several roles, is one entry.
pub fn key_map(keys: &[&PublicKey]) -> Value {
    Value::Object(keys.iter().map(|k| (k.id(), k.public())).collect())
}

/// A role signed by `key` alone.
pub fn role(key: &PublicKey) -> Value {
    json!({ "keyids": [key.id()], "threshold": 1 })
}

/// `signed` with its one signature, by `key`, as canonical JSON.
pub fn envelope(signed: &Value, key: &Key) -> Result<Vec<u8>, String> {
    let sig = key.sign_bytes(&canonical(signed)?);
    signed_envelope(signed, &key.public_key(), &sig)
}

/// `signed` with `sig`, made elsewhere over its canonical bytes by `key`'s
/// private half, as canonical JSON. The signature is not checked here.
pub fn signed_envelope(signed: &Value, key: &PublicKey, sig: &[u8; 64]) -> Result<Vec<u8>, String> {
    let signature = json!({ "keyid": key.id(), "sig": hex_bytes(sig) });
    canonical(&json!({ "signed": signed, "signatures": [signature] }))
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
mod tests;
