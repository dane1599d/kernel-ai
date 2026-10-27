//! A repository the tests sign and serve from memory, with Ed25519 and P-256
//! signers, and the client's remote and cache over it.

#![allow(dead_code)]

use kaituf::Error;
use kaituf::updater::{Cache, Remote};
use p256::ecdsa::signature::Signer as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const NOW: u64 = 1_800_000_000;
pub const LATER: &str = "2030-01-01T00:00:00Z";
pub const PAST: &str = "2020-01-01T00:00:00Z";

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A key the tests sign with.
#[derive(Clone)]
pub struct Signer {
    pub public: Value,
    kind: Kind,
}

#[derive(Clone)]
enum Kind {
    Ed25519(ed25519_compact::KeyPair),
    /// With a high s when the bool is set, as Python's cryptography may sign.
    P256(p256::ecdsa::SigningKey, bool),
}

/// The SubjectPublicKeyInfo prefix of a P-256 key.
const SPKI: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(match i <= chunk.len() {
                true => char::from(ABC[(n >> (18 - 6 * i) & 63) as usize]),
                false => '=',
            });
        }
    }
    out
}

/// A minimal DER INTEGER of a 32-byte big-endian number.
fn der_int(bytes: &[u8]) -> Vec<u8> {
    let start = bytes.iter().position(|&b| b != 0).unwrap_or(31);
    let mut value = bytes[start..].to_vec();
    if value[0] & 0x80 != 0 {
        value.insert(0, 0);
    }
    let mut out = vec![0x02, value.len() as u8];
    out.extend(value);
    out
}

impl Signer {
    pub fn ed25519(seed: u8) -> Self {
        let pair = ed25519_compact::KeyPair::from_seed(ed25519_compact::Seed::new([seed; 32]));
        let public = json!({ "keytype": "ed25519", "scheme": "ed25519", "keyval": { "public": hex(pair.pk.as_ref()) } });
        Signer {
            public,
            kind: Kind::Ed25519(pair),
        }
    }

    /// P-256, written as a PEM key (`pem`) or as SEC1 hex, signing with a high
    /// s if `high_s`.
    pub fn p256(seed: u8, pem: bool, high_s: bool) -> Self {
        let key = p256::ecdsa::SigningKey::from_slice(&[seed; 32]).unwrap();
        let point = key.verifying_key().to_sec1_point(false);
        let public = match pem {
            true => {
                let mut der = SPKI.to_vec();
                der.extend_from_slice(point.as_bytes());
                format!(
                    "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
                    base64(&der)
                )
            }
            false => hex(point.as_bytes()),
        };
        let public = json!({ "keytype": "ecdsa", "scheme": "ecdsa-sha2-nistp256", "keyval": { "public": public } });
        Signer {
            public,
            kind: Kind::P256(key, high_s),
        }
    }

    /// A key of a type the client does not check.
    pub fn p384_public() -> Value {
        json!({ "keytype": "ecdsa", "scheme": "ecdsa-sha2-nistp384", "keyval": { "public": "-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----\n" } })
    }

    pub fn id(&self) -> String {
        key_id(&self.public)
    }

    pub fn sign(&self, message: &[u8]) -> String {
        match &self.kind {
            Kind::Ed25519(pair) => hex(pair.sk.sign(message, None).as_ref()),
            Kind::P256(key, high_s) => {
                let sig: p256::ecdsa::Signature = key.sign(message);
                let sig = sig.normalize_s();
                let (r, s) = (sig.r(), sig.s());
                let s = match high_s {
                    true => -*s.as_ref(),
                    false => *s.as_ref(),
                };
                let (r, s) = (r.to_bytes(), s.to_bytes());
                let body = [der_int(&r), der_int(&s)].concat();
                let mut der = vec![0x30, body.len() as u8];
                der.extend(body);
                hex(&der)
            }
        }
    }
}

pub fn key_id(public: &Value) -> String {
    hex(&Sha256::digest(
        kaituf::canonical::canonical(public).unwrap(),
    ))
}

/// `signed` as metadata, signed by `signers`.
pub fn envelope(signed: &Value, signers: &[&Signer]) -> Vec<u8> {
    let message = kaituf::canonical::canonical(signed).unwrap();
    let sigs: Vec<Value> = signers
        .iter()
        .map(|s| json!({ "keyid": s.id(), "sig": s.sign(&message) }))
        .collect();
    serde_json::to_vec(&json!({ "signed": signed, "signatures": sigs })).unwrap()
}

/// A delegation: the role's name, its signer, its paths, whether it is terminating.
pub type Delegation = (String, Signer, Vec<String>, bool);

/// A repository: one signer per top-level role unless a test changes them,
/// consistent snapshots, and what it has published, by file name.
pub struct Repo {
    pub signers: BTreeMap<String, Vec<Signer>>,
    pub thresholds: BTreeMap<String, u64>,
    pub version: BTreeMap<String, u64>,
    pub expires: BTreeMap<String, String>,
    pub consistent: bool,
    pub targets: BTreeMap<String, Value>,
    pub artifacts: BTreeMap<String, Vec<u8>>,
    /// Delegations of a targets role.
    pub delegations: BTreeMap<String, Vec<Delegation>>,
    /// Targets of a delegated role.
    pub delegated_targets: BTreeMap<String, BTreeMap<String, Value>>,
    pub files: BTreeMap<String, Vec<u8>>,
    pub hashes_in_meta: bool,
}

impl Repo {
    pub fn new() -> Self {
        let mut repo = Repo {
            signers: BTreeMap::new(),
            thresholds: BTreeMap::new(),
            version: BTreeMap::new(),
            expires: BTreeMap::new(),
            consistent: true,
            targets: BTreeMap::new(),
            artifacts: BTreeMap::new(),
            delegations: BTreeMap::new(),
            delegated_targets: BTreeMap::new(),
            files: BTreeMap::new(),
            hashes_in_meta: false,
        };
        for (i, role) in ["root", "timestamp", "snapshot", "targets"]
            .into_iter()
            .enumerate()
        {
            repo.signers
                .insert(role.into(), vec![Signer::ed25519(10 + i as u8)]);
            repo.thresholds.insert(role.into(), 1);
            repo.expires.insert(role.into(), LATER.into());
        }
        repo.publish_all();
        repo
    }

    pub fn root_signed(&self) -> Value {
        let mut keys = serde_json::Map::new();
        let mut roles = serde_json::Map::new();
        for role in ["root", "timestamp", "snapshot", "targets"] {
            let signers = &self.signers[role];
            for s in signers {
                keys.insert(s.id(), s.public.clone());
            }
            let ids: Vec<String> = signers.iter().map(Signer::id).collect();
            roles.insert(
                role.into(),
                json!({ "keyids": ids, "threshold": self.thresholds[role] }),
            );
        }
        json!({
            "_type": "root", "spec_version": "1.0.31", "version": self.version.get("root").copied().unwrap_or(1),
            "expires": self.expires["root"], "consistent_snapshot": self.consistent, "keys": keys, "roles": roles,
        })
    }

    /// Root at its next version, signed by `old` signers and the current ones.
    pub fn publish_root(&mut self, old: &[Signer]) {
        let v = self.version.get("root").map_or(1, |v| v + 1);
        self.version.insert("root".into(), v);
        let mut signers: Vec<&Signer> = old.iter().collect();
        for s in &self.signers["root"] {
            if !signers.iter().any(|o| o.id() == s.id()) {
                signers.push(s);
            }
        }
        let bytes = envelope(&self.root_signed(), &signers);
        self.files.insert(format!("{v}.root.json"), bytes);
    }

    fn targets_signed(&self, role: &str, version: u64) -> Value {
        let targets = match role {
            "targets" => self.targets.clone(),
            _ => self
                .delegated_targets
                .get(role)
                .cloned()
                .unwrap_or_default(),
        };
        let mut signed = json!({
            "_type": "targets", "spec_version": "1.0.31", "version": version,
            "expires": self.expires.get(role).cloned().unwrap_or(LATER.into()), "targets": targets,
        });
        if let Some(children) = self.delegations.get(role) {
            let keys: serde_json::Map<String, Value> = children
                .iter()
                .map(|(_, s, _, _)| (s.id(), s.public.clone()))
                .collect();
            let roles: Vec<Value> = children
                .iter()
                .map(|(name, s, paths, term)| json!({ "name": name, "keyids": [s.id()], "threshold": 1, "terminating": term, "paths": paths }))
                .collect();
            signed["delegations"] = json!({ "keys": keys, "roles": roles });
        }
        signed
    }

    /// The signers of targets role `role`: top-level from root, delegated from
    /// whoever delegates it.
    fn role_signers(&self, role: &str) -> Vec<Signer> {
        if let Some(s) = self.signers.get(role) {
            return s.clone();
        }
        let mut found = Vec::new();
        for children in self.delegations.values() {
            for (name, signer, _, _) in children {
                if name == role && !found.iter().any(|f: &Signer| f.id() == signer.id()) {
                    found.push(signer.clone());
                }
            }
        }
        found
    }

    fn file_name(&self, role: &str, version: u64) -> String {
        match self.consistent {
            true => format!("{version}.{}.json", kaituf::quote(role)),
            false => format!("{}.json", kaituf::quote(role)),
        }
    }

    fn meta_entry(&self, name: &str, version: u64) -> Value {
        let mut entry = json!({ "version": version });
        if self.hashes_in_meta {
            let bytes = &self.files[name];
            entry["length"] = json!(bytes.len());
            entry["hashes"] = json!({ "sha256": hex(&Sha256::digest(bytes)) });
        }
        entry
    }

    /// Publishes every targets role named, then snapshot and timestamp, each at
    /// its next version.
    pub fn publish(&mut self, roles: &[&str]) {
        for role in roles {
            let v = self.version.get(*role).map_or(1, |v| v + 1);
            self.version.insert((*role).into(), v);
            let signed = self.targets_signed(role, v);
            let signers = self.role_signers(role);
            let bytes = envelope(&signed, &signers.iter().collect::<Vec<_>>());
            self.files.insert(self.file_name(role, v), bytes);
        }
        let mut meta = serde_json::Map::new();
        let targets_roles: Vec<String> = self
            .version
            .keys()
            .filter(|r| !["root", "timestamp", "snapshot"].contains(&r.as_str()))
            .cloned()
            .collect();
        for role in targets_roles {
            let v = self.version[&role];
            meta.insert(
                format!("{role}.json"),
                self.meta_entry(&self.file_name(&role, v), v),
            );
        }
        let v = self.version.get("snapshot").map_or(1, |v| v + 1);
        self.version.insert("snapshot".into(), v);
        let snapshot = json!({ "_type": "snapshot", "spec_version": "1.0.31", "version": v, "expires": self.expires["snapshot"], "meta": meta });
        let name = match self.consistent {
            true => format!("{v}.snapshot.json"),
            false => "snapshot.json".into(),
        };
        let signers = self.signers["snapshot"].clone();
        self.files.insert(
            name.clone(),
            envelope(&snapshot, &signers.iter().collect::<Vec<_>>()),
        );
        self.publish_timestamp(self.meta_entry(&name, v));
    }

    pub fn publish_timestamp(&mut self, snapshot: Value) {
        let v = self.version.get("timestamp").map_or(1, |v| v + 1);
        self.version.insert("timestamp".into(), v);
        let signed = json!({ "_type": "timestamp", "spec_version": "1.0.31", "version": v, "expires": self.expires["timestamp"], "meta": { "snapshot.json": snapshot } });
        let signers = self.signers["timestamp"].clone();
        self.files.insert(
            "timestamp.json".into(),
            envelope(&signed, &signers.iter().collect::<Vec<_>>()),
        );
    }

    fn publish_all(&mut self) {
        self.publish_root(&[]);
        self.publish(&["targets"]);
    }

    /// Adds an artifact to targets role `role` (published by the next publish).
    pub fn add_artifact(&mut self, role: &str, path: &str, bytes: &[u8]) {
        let entry =
            json!({ "length": bytes.len(), "hashes": { "sha256": hex(&Sha256::digest(bytes)) } });
        match role {
            "targets" => self.targets.insert(path.into(), entry),
            _ => self
                .delegated_targets
                .entry(role.into())
                .or_default()
                .insert(path.into(), entry),
        };
        self.artifacts.insert(path.into(), bytes.to_vec());
    }

    pub fn delegate(
        &mut self,
        from: &str,
        name: &str,
        signer: Signer,
        paths: &[&str],
        terminating: bool,
    ) {
        let paths = paths.iter().map(|p| p.to_string()).collect();
        self.delegations.entry(from.into()).or_default().push((
            name.into(),
            signer,
            paths,
            terminating,
        ));
    }

    pub fn initial_root(&self) -> Vec<u8> {
        self.files["1.root.json"].clone()
    }
}

/// The client's side: a remote over a repository's files, logging each name
/// fetched, and a cache in memory.
pub struct Served<'a> {
    pub repo: &'a Repo,
    pub log: Vec<String>,
}

impl Remote for Served<'_> {
    fn fetch(&mut self, name: &str, _max: u64) -> Result<Option<Vec<u8>>, Error> {
        self.log.push(name.into());
        Ok(self.repo.files.get(name).cloned())
    }
}

#[derive(Default, Clone)]
pub struct Memory(pub BTreeMap<String, Vec<u8>>);

impl Cache for Memory {
    fn load(&mut self, role: &str) -> Option<Vec<u8>> {
        self.0.get(role).cloned()
    }

    fn save(&mut self, role: &str, bytes: &[u8]) -> Result<(), Error> {
        self.0.insert(role.into(), bytes.to_vec());
        Ok(())
    }
}

/// A cache holding only `repo`'s first root.
pub fn fresh(repo: &Repo) -> Memory {
    let mut cache = Memory::default();
    cache.0.insert("root".into(), repo.initial_root());
    cache
}

/// One refresh of `cache` from `repo`: its result and the names fetched.
pub fn refresh(repo: &Repo, cache: &mut Memory, now: u64) -> (Result<(), Error>, Vec<String>) {
    let mut remote = Served {
        repo,
        log: Vec::new(),
    };
    let result =
        kaituf::updater::Updater::new(&mut remote, cache, now).and_then(|mut u| u.refresh());
    (result, remote.log)
}

/// The version of the role `cache` holds.
pub fn cached_version(cache: &Memory, role: &str) -> Option<u64> {
    let bytes = cache.0.get(role)?;
    serde_json::from_slice::<Value>(bytes).ok()?["signed"]["version"].as_u64()
}
