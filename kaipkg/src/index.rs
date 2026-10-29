//! A package's index entry (ADR 0011 §3): the `custom.kai` fields its signed
//! TUF target carries, read before its bytes are. Each field is required but
//! `world`, which only an `app` has, and no other is allowed. Once the bytes are
//! here, [`Index::matches`] holds cargo's rule: the package's own header must say
//! what its entry says.

use crate::Sha;
use crate::header::{Depends, Header};
use crate::resolve::Entry;
use crate::version::Version;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kaituf::meta::TargetFile;
use serde_json::{Map, Value};

/// `custom.kai`'s fields, in the packer's order.
const FIELDS: [&str; 10] = [
    "name", "version", "kind", "summary", "licence", "depends", "grants", "actions", "yanked",
    "world",
];

/// One target's index entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Index {
    pub name: String,
    pub version: Version,
    pub kind: String,
    /// Data, never instructions.
    pub summary: String,
    pub licence: String,
    pub world: Option<String>,
    /// As written, `<name> <range>` each.
    pub depends: Vec<String>,
    pub grants: Vec<String>,
    pub actions: Vec<String>,
    pub yanked: bool,
    /// The target's SHA-256 and length.
    pub sha: Sha,
    pub size: u64,
}

impl Index {
    /// `target`'s `custom.kai` fields, parsed strictly.
    pub fn parse(target: &TargetFile) -> Result<Self, String> {
        let kai = target
            .custom
            .get("kai")
            .and_then(Value::as_object)
            .ok_or("it has no custom.kai")?;
        if let Some(key) = kai.keys().find(|key| !FIELDS.contains(&key.as_str())) {
            return Err(format!("custom.kai has an unknown field {key}"));
        }
        let text = |key: &str| -> Result<String, String> {
            kai.get(key)
                .and_then(Value::as_str)
                .map(String::from)
                .ok_or_else(|| not_a(key, "string"))
        };
        let version = text("version")?;
        let world = match kai.get("world") {
            None => None,
            Some(value) => Some(
                value
                    .as_str()
                    .ok_or_else(|| not_a("world", "string"))?
                    .into(),
            ),
        };
        Ok(Index {
            name: text("name")?,
            version: Version::parse(&version)
                .map_err(|why| format!("custom.kai.version: {why}"))?,
            kind: text("kind")?,
            summary: text("summary")?,
            licence: text("licence")?,
            world,
            depends: strings(kai, "depends")?,
            grants: strings(kai, "grants")?,
            actions: strings(kai, "actions")?,
            yanked: kai
                .get("yanked")
                .and_then(Value::as_bool)
                .ok_or_else(|| not_a("yanked", "boolean"))?,
            sha: sha256(target)?,
            size: target.length,
        })
    }

    /// The name a target with this entry has: `<name>-<version>.kpkg`.
    pub fn path(&self) -> String {
        format!("{}-{}.kpkg", self.name, self.version)
    }

    /// Whether `target` is named as this entry says it must be.
    pub fn check_path(&self, target: &TargetFile) -> Result<(), String> {
        let path = self.path();
        match target.path == path {
            true => Ok(()),
            false => Err(format!("it is named {}, not {path}", target.path)),
        }
    }

    /// Cargo's rule: `header`, whose text is `text`, says what this entry says.
    pub fn matches(&self, header: &Header, text: &str) -> Result<(), String> {
        let version = self.version.to_string();
        let own_version = header.version.to_string();
        let same = [
            ("name", self.name.as_str(), header.name.as_str()),
            ("version", version.as_str(), own_version.as_str()),
            ("kind", self.kind.as_str(), header.kind.name()),
            ("summary", self.summary.as_str(), header.summary.as_str()),
            ("licence", self.licence.as_str(), header.licence.as_str()),
        ];
        if let Some((key, ..)) = same.iter().find(|(_, indexed, own)| indexed != own) {
            return Err(format!("its header's {key} is not its index entry's"));
        }
        if self.world.as_deref() != header.world.as_deref() {
            return Err("its header's world is not its index entry's".into());
        }
        let depends: Vec<&str> = text
            .lines()
            .filter_map(|line| line.strip_prefix("depends "))
            .collect();
        if self.depends != depends {
            return Err("its header's depends are not its index entry's".into());
        }
        Ok(())
    }

    /// What resolution and the store need of it.
    pub fn entry(&self) -> Result<Entry, String> {
        let depends = self
            .depends
            .iter()
            .map(|text| Depends::parse(text).map_err(|why| format!("custom.kai.depends: {why}")))
            .collect::<Result<_, _>>()?;
        Ok(Entry {
            name: self.name.clone(),
            version: self.version,
            sha: self.sha,
            size: self.size,
            depends,
            grants: self.grants.clone(),
            yanked: self.yanked,
        })
    }
}

fn not_a(key: &str, what: &str) -> String {
    format!("custom.kai.{key} is not a {what}")
}

fn strings(kai: &Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    let list = kai
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| not_a(key, "list"))?;
    list.iter()
        .map(|item| {
            item.as_str()
                .map(String::from)
                .ok_or_else(|| not_a(key, "list of strings"))
        })
        .collect()
}

/// The SHA-256 `target` names, which every index entry has.
fn sha256(target: &TargetFile) -> Result<Sha, String> {
    target
        .hashes
        .iter()
        .find(|(algorithm, _)| algorithm == "sha256")
        .and_then(|(_, digest)| Sha::try_from(digest.as_slice()).ok())
        .ok_or_else(|| format!("{} names no SHA-256", target.path))
}
