//! TUF metadata (spec §4), parsed strictly where the spec and python-tuf are
//! strict and kept whole where they are not: fields a reader does not know are
//! kept in the signed value, so they are signed over like any other.
//!
//! [`Meta::parse`] reads the envelope: `signed`, its canonical bytes, and the
//! signatures, whose key ids must be unique. Each role type then reads its own
//! fields from `signed`.

use crate::canonical::canonical;
use crate::keys::Key;
use crate::{Error, malformed};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256, Sha512};

pub const ROLES: [&str; 4] = ["root", "timestamp", "snapshot", "targets"];

/// A metadata file: its signed part, typed, with what verifying it needs.
#[derive(Clone, Debug)]
pub struct Meta<T> {
    pub signed: T,
    pub version: u64,
    /// Seconds since 1970 UTC.
    pub expires: u64,
    /// The canonical bytes of `signed`, which the signatures cover.
    pub canonical: Vec<u8>,
    /// Key id and hex signature, each key id once.
    pub sigs: Vec<(String, String)>,
}

/// A role's keys and threshold, from root or from a delegation.
#[derive(Clone, Debug)]
pub struct Role {
    pub keyids: Vec<String>,
    pub threshold: u64,
}

#[derive(Clone, Debug)]
pub struct Root {
    pub consistent_snapshot: bool,
    pub keys: BTreeMap<String, Key>,
    /// Each of [`ROLES`].
    pub roles: BTreeMap<String, Role>,
}

/// A metadata file as snapshot and timestamp name it.
#[derive(Clone, Debug)]
pub struct MetaFile {
    pub version: u64,
    pub length: Option<u64>,
    pub hashes: Option<Hashes>,
}

#[derive(Clone, Debug)]
pub struct Timestamp {
    pub snapshot: MetaFile,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    /// By file name, `<role>.json`.
    pub meta: BTreeMap<String, MetaFile>,
}

/// Algorithm and digest, as the metadata names them; never empty.
pub type Hashes = Vec<(String, Vec<u8>)>;

/// An artifact a targets role names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetFile {
    pub path: String,
    pub length: u64,
    pub hashes: Hashes,
    /// `custom`, or null: data such as `custom.kai`'s index fields.
    pub custom: Value,
}

#[derive(Clone, Debug)]
pub struct Targets {
    pub targets: BTreeMap<String, TargetFile>,
    pub delegations: Option<Delegations>,
}

#[derive(Clone, Debug)]
pub struct Delegations {
    pub keys: BTreeMap<String, Key>,
    /// In the order the metadata lists them, which is the search order.
    pub roles: Vec<DelegatedRole>,
}

#[derive(Clone, Debug)]
pub struct DelegatedRole {
    pub name: String,
    pub role: Role,
    pub terminating: bool,
    pub paths: Paths,
}

#[derive(Clone, Debug)]
pub enum Paths {
    /// Shell-style patterns, matched one `/`-separated segment at a time.
    Patterns(Vec<String>),
    /// Hex prefixes of the SHA-256 of the target path (hashed bins).
    HashPrefixes(Vec<String>),
}

impl<T> Meta<T> {
    /// The envelope and common fields of `bytes`, if its `_type` is `kind`; `read`
    /// then types the rest of `signed`.
    pub fn parse(
        bytes: &[u8],
        kind: &str,
        read: impl FnOnce(&Map<String, Value>) -> Result<T, Error>,
    ) -> Result<Self, Error> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|e| Error::Malformed(e.to_string()))?;
        let signed = value["signed"]
            .as_object()
            .ok_or(Error::Malformed("no signed object".into()))?;
        let mut sigs: Vec<(String, String)> = Vec::new();
        for sig in value["signatures"]
            .as_array()
            .ok_or(Error::Malformed("no signatures".into()))?
        {
            let (Some(keyid), Some(hex)) = (sig["keyid"].as_str(), sig["sig"].as_str()) else {
                return malformed("a signature is not a keyid and a sig");
            };
            if sigs.iter().any(|(k, _)| k == keyid) {
                return malformed(alloc::format!("two signatures by key {keyid}"));
            }
            sigs.push((keyid.into(), hex.into()));
        }
        if signed.get("_type").and_then(Value::as_str) != Some(kind) {
            return malformed(alloc::format!("not {kind} metadata"));
        }
        let spec = string(signed, "spec_version")?;
        let parts: Vec<&str> = spec.split('.').collect();
        let numeric = parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
        if !(2..=3).contains(&parts.len()) || !numeric || parts[0] != "1" {
            return malformed(alloc::format!("unsupported spec_version {spec}"));
        }
        Ok(Meta {
            version: positive(signed, "version")?,
            expires: time(string(signed, "expires")?)?,
            canonical: canonical(&value["signed"])?,
            signed: read(signed)?,
            sigs,
        })
    }
}

fn string<'a>(map: &'a Map<String, Value>, key: &str) -> Result<&'a str, Error> {
    map.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Malformed(alloc::format!("no {key} string")))
}

fn object<'a>(map: &'a Map<String, Value>, key: &str) -> Result<&'a Map<String, Value>, Error> {
    map.get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| Error::Malformed(alloc::format!("no {key} object")))
}

/// A whole number of at least 1 (JSON booleans are not numbers here).
fn positive(map: &Map<String, Value>, key: &str) -> Result<u64, Error> {
    match map.get(key).and_then(Value::as_u64) {
        Some(n) if n >= 1 => Ok(n),
        _ => malformed(alloc::format!("{key} is not a positive integer")),
    }
}

fn strings(value: &Value, what: &str) -> Result<Vec<String>, Error> {
    value
        .as_array()
        .and_then(|items| items.iter().map(|v| v.as_str().map(String::from)).collect())
        .ok_or_else(|| Error::Malformed(alloc::format!("{what} is not a list of strings")))
}

/// `YYYY-MM-DDTHH:MM:SSZ` as seconds since 1970 UTC, as python-tuf reads it.
pub fn time(text: &str) -> Result<u64, Error> {
    let b = text.as_bytes();
    let shaped = b.len() == 20
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            19 => *c == b'Z',
            _ => c.is_ascii_digit(),
        });
    if !shaped {
        return malformed(alloc::format!(
            "expires {text:?} is not YYYY-MM-DDTHH:MM:SSZ"
        ));
    }
    let n = |from: usize, to: usize| -> u64 { text[from..to].parse().unwrap_or(0) };
    let (y, m, d, hh, mm, ss) = (n(0, 4), n(5, 7), n(8, 10), n(11, 13), n(14, 16), n(17, 19));
    let leap = (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400);
    let days_in = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let real = (1970..=9999).contains(&y)
        && (1..=12).contains(&m)
        && (1..=days_in[m as usize - 1]).contains(&d)
        && hh < 24
        && mm < 60
        && ss < 60;
    if !real {
        return malformed(alloc::format!("expires {text:?} names no real moment"));
    }
    // Howard Hinnant's days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let (era, yoe) = (y / 400, y % 400);
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Ok(days * 86_400 + hh * 3600 + mm * 60 + ss)
}

fn keys(map: &Map<String, Value>) -> Result<BTreeMap<String, Key>, Error> {
    map.iter()
        .map(|(id, entry)| {
            Ok((
                id.clone(),
                Key::parse(entry).map_err(|w| Error::Malformed(w.into()))?,
            ))
        })
        .collect()
}

/// A role's `keyids` (unique, each one of `keys`) and `threshold` (at least 1).
fn role(value: &Value, keys: &BTreeMap<String, Key>, name: &str) -> Result<Role, Error> {
    let map = value
        .as_object()
        .ok_or_else(|| Error::Malformed(alloc::format!("role {name} is not an object")))?;
    let keyids = strings(&value["keyids"], "keyids")?;
    for (i, id) in keyids.iter().enumerate() {
        if keyids[..i].contains(id) {
            return malformed(alloc::format!("role {name} lists key {id} twice"));
        }
        if !keys.contains_key(id) {
            return malformed(alloc::format!(
                "role {name} names key {id}, which is not listed"
            ));
        }
    }
    Ok(Role {
        keyids,
        threshold: positive(map, "threshold")?,
    })
}

impl Root {
    pub fn parse(bytes: &[u8]) -> Result<Meta<Root>, Error> {
        Meta::parse(bytes, "root", |signed| {
            let consistent_snapshot = match signed.get("consistent_snapshot") {
                None => false,
                Some(Value::Bool(b)) => *b,
                Some(_) => return malformed("consistent_snapshot is not a boolean"),
            };
            let keys = keys(object(signed, "keys")?)?;
            let listed = object(signed, "roles")?;
            if listed.len() != ROLES.len() || ROLES.iter().any(|r| !listed.contains_key(*r)) {
                return malformed("roles are not root, timestamp, snapshot and targets");
            }
            let roles = listed
                .iter()
                .map(|(name, value)| Ok((name.clone(), role(value, &keys, name)?)))
                .collect::<Result<_, Error>>()?;
            Ok(Root {
                consistent_snapshot,
                keys,
                roles,
            })
        })
    }

    pub fn role(&self, name: &str) -> &Role {
        // Parsing made sure every top-level role is there.
        &self.roles[name]
    }
}

impl MetaFile {
    fn parse(value: &Value) -> Result<Self, Error> {
        let map = value
            .as_object()
            .ok_or(Error::Malformed("a meta entry is not an object".into()))?;
        let length = match map.get("length") {
            None => None,
            Some(v) => Some(
                v.as_u64()
                    .ok_or(Error::Malformed("a length is not a whole number".into()))?,
            ),
        };
        let hashes = map.get("hashes").map(hashes).transpose()?;
        Ok(MetaFile {
            version: positive(map, "version")?,
            length,
            hashes,
        })
    }

    /// Whether `bytes` have the length and hashes named, where they are named.
    pub fn check(&self, bytes: &[u8], what: &str) -> Result<(), Error> {
        if self.length.is_some_and(|n| n != bytes.len() as u64) {
            return Err(Error::Hash(alloc::format!(
                "{what} is {} bytes, not {:?}",
                bytes.len(),
                self.length
            )));
        }
        match &self.hashes {
            Some(hashes) => check_hashes(hashes, bytes, what),
            None => Ok(()),
        }
    }
}

fn hashes(value: &Value) -> Result<Hashes, Error> {
    let map = value.as_object().filter(|m| !m.is_empty());
    let map = map.ok_or(Error::Malformed("hashes are not a non-empty object".into()))?;
    map.iter()
        .map(|(alg, digest)| {
            let digest = digest.as_str().and_then(crate::unhex);
            Ok((
                alg.clone(),
                digest.ok_or(Error::Malformed("a hash is not hex".into()))?,
            ))
        })
        .collect()
}

/// Every hash named must be one this client computes, and match.
pub fn check_hashes(hashes: &Hashes, bytes: &[u8], what: &str) -> Result<(), Error> {
    for (alg, want) in hashes {
        let got: Vec<u8> = match alg.as_str() {
            "sha256" => Sha256::digest(bytes).to_vec(),
            "sha512" => Sha512::digest(bytes).to_vec(),
            other => {
                return Err(Error::Hash(alloc::format!(
                    "{what}: unknown hash algorithm {other}"
                )));
            }
        };
        if got != *want {
            return Err(Error::Hash(alloc::format!(
                "{what}: its {alg} is not the one named"
            )));
        }
    }
    Ok(())
}

impl Timestamp {
    pub fn parse(bytes: &[u8]) -> Result<Meta<Timestamp>, Error> {
        Meta::parse(bytes, "timestamp", |signed| {
            let meta = object(signed, "meta")?;
            let snapshot = meta
                .get("snapshot.json")
                .ok_or(Error::Malformed("no snapshot.json in meta".into()))?;
            Ok(Timestamp {
                snapshot: MetaFile::parse(snapshot)?,
            })
        })
    }
}

impl Snapshot {
    pub fn parse(bytes: &[u8]) -> Result<Meta<Snapshot>, Error> {
        Meta::parse(bytes, "snapshot", |signed| {
            let meta = object(signed, "meta")?
                .iter()
                .map(|(name, entry)| Ok((name.clone(), MetaFile::parse(entry)?)))
                .collect::<Result<_, Error>>()?;
            Ok(Snapshot { meta })
        })
    }
}

impl Targets {
    pub fn parse(bytes: &[u8]) -> Result<Meta<Targets>, Error> {
        Meta::parse(bytes, "targets", |signed| {
            let mut targets = BTreeMap::new();
            for (path, entry) in object(signed, "targets")? {
                let map = entry
                    .as_object()
                    .ok_or(Error::Malformed("a target is not an object".into()))?;
                let length = map.get("length").and_then(Value::as_u64);
                let length = length.ok_or(Error::Malformed("a target has no length".into()))?;
                let hashes = hashes(map.get("hashes").unwrap_or(&Value::Null))?;
                let custom = map.get("custom").cloned().unwrap_or(Value::Null);
                targets.insert(
                    path.clone(),
                    TargetFile {
                        path: path.clone(),
                        length,
                        hashes,
                        custom,
                    },
                );
            }
            let delegations = match signed.get("delegations") {
                None => None,
                Some(value) => Some(Delegations::parse(value)?),
            };
            Ok(Targets {
                targets,
                delegations,
            })
        })
    }
}

impl Delegations {
    fn parse(value: &Value) -> Result<Self, Error> {
        let map = value
            .as_object()
            .ok_or(Error::Malformed("delegations are not an object".into()))?;
        if map.contains_key("succinct_roles") {
            return malformed("succinct_roles (TAP 15) is not supported");
        }
        let keys = keys(object(map, "keys")?)?;
        let listed = map.get("roles").and_then(Value::as_array);
        let mut roles: Vec<DelegatedRole> = Vec::new();
        for entry in listed.ok_or(Error::Malformed("delegations have no roles".into()))? {
            let fields = entry
                .as_object()
                .ok_or(Error::Malformed("a delegated role is not an object".into()))?;
            let name = string(fields, "name")?;
            if name.is_empty() || ROLES.contains(&name) || roles.iter().any(|r| r.name == name) {
                return malformed(alloc::format!(
                    "delegated role name {name:?} is empty, top-level or repeated"
                ));
            }
            let terminating = fields.get("terminating").and_then(Value::as_bool);
            let paths = match (fields.get("paths"), fields.get("path_hash_prefixes")) {
                (Some(p), None) => Paths::Patterns(strings(p, "paths")?),
                (None, Some(p)) => Paths::HashPrefixes(strings(p, "path_hash_prefixes")?),
                _ => {
                    return malformed("a delegated role needs one of paths and path_hash_prefixes");
                }
            };
            roles.push(DelegatedRole {
                name: name.into(),
                role: role(entry, &keys, name)?,
                terminating: terminating
                    .ok_or(Error::Malformed("terminating is not a boolean".into()))?,
                paths,
            });
        }
        Ok(Delegations { keys, roles })
    }
}

impl DelegatedRole {
    /// Whether this role is trusted for `path`.
    pub fn matches(&self, path: &str) -> bool {
        match &self.paths {
            Paths::Patterns(patterns) => patterns.iter().any(|p| path_matches(p, path)),
            Paths::HashPrefixes(prefixes) => {
                let digest = crate::hex(&Sha256::digest(path.as_bytes()));
                prefixes.iter().any(|p| digest.starts_with(p.as_str()))
            }
        }
    }
}

/// python-tuf's rule: the same number of `/`-separated segments, each matching
/// its pattern segment as `fnmatch.fnmatchcase` does, so `*` never crosses a `/`.
pub fn path_matches(pattern: &str, path: &str) -> bool {
    let (p, t): (Vec<&str>, Vec<&str>) = (pattern.split('/').collect(), path.split('/').collect());
    p.len() == t.len()
        && p.iter()
            .zip(&t)
            .all(|(p, t)| fnmatch(p.as_bytes(), t.as_bytes()))
}

/// `*`, `?`, `[set]` and `[!set]` with ranges; a `[` with no closing `]` is itself.
fn fnmatch(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => (0..=text.len()).any(|i| fnmatch(rest, &text[i..])),
        Some((b'?', rest)) => !text.is_empty() && fnmatch(rest, &text[1..]),
        Some((b'[', rest)) => match class(rest) {
            Some((set, after)) => text
                .split_first()
                .is_some_and(|(&c, t)| set(c) && fnmatch(after, t)),
            None => text.first() == Some(&b'[') && fnmatch(rest, &text[1..]),
        },
        Some((&c, rest)) => text.first() == Some(&c) && fnmatch(rest, &text[1..]),
    }
}

/// The set a `[...]` after its `[` stands for, and the pattern after its `]`.
fn class(pattern: &[u8]) -> Option<(impl Fn(u8) -> bool + '_, &[u8])> {
    let (negated, body) = match pattern.first() {
        Some(b'!') => (true, &pattern[1..]),
        _ => (false, pattern),
    };
    // A `]` first in the set is a member, as in Python.
    let end = body.iter().skip(1).position(|&b| b == b']')? + 1;
    let set = &body[..end];
    let member = move |c: u8| {
        let mut i = 0;
        let mut hit = false;
        while i < set.len() {
            if i + 2 < set.len() && set[i + 1] == b'-' {
                hit |= (set[i]..=set[i + 2]).contains(&c);
                i += 3;
            } else {
                hit |= set[i] == c;
                i += 1;
            }
        }
        hit != negated
    };
    Some((member, &body[end + 1..]))
}

#[cfg(test)]
mod tests;
