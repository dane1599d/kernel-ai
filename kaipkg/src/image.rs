//! The image's signed repository (ADR 0011 §3, §8; P4 task 1): the built-in
//! packages the slot image carries, checked with `kaituf`, become generation 1.
//!
//! The repository is `cargo xtask pkgrepo build`'s, as files by path:
//! `metadata/1.root.json` and the other roles beside it, and each package at
//! `targets/<sha256>.<name>-<version>.kpkg`. The image's own `1.root.json` is
//! the root it is checked against: the image carries its root as it carries the
//! kernel, and a `dev` image's is a throwaway test root (P4 plan §4, item 3).
//!
//! Metadata that does not verify refuses the whole repository. Each target the
//! [`ROLE`] role lists is then checked on its own: its length and SHA-256, then
//! its own header against its `custom.kai` index entry (cargo's rule). One that
//! fails is refused by name and left out; the rest are kept.

use crate::Sha;
use crate::bundle::{self, HEADER};
use crate::header::Header;
use crate::resolve::{self, Entry, Want};
use crate::version::Version;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kaituf::meta::TargetFile;
use kaituf::updater::{self, Cache, Remote, Updater};
use serde_json::{Map, Value};

/// The role that lists every package (ADR 0011 §3), delegated by `targets`.
pub const ROLE: &str = "packages";
pub const METADATA: &str = "metadata";
pub const TARGETS: &str = "targets";
/// The root the image carries.
pub const ROOT: &str = "metadata/1.root.json";

/// `custom.kai`'s fields, in the packer's order: every one is required but
/// `world`, which only an `app` has, and no other is allowed.
const FIELDS: [&str; 10] = [
    "name", "version", "kind", "summary", "licence", "depends", "grants", "actions", "yanked",
    "world",
];

/// The repository's files.
pub trait Files {
    /// The file at `path`, at most `max` bytes; None when there is none, and an
    /// error when it is longer.
    fn file(&mut self, path: &str, max: u64) -> Result<Option<Vec<u8>>, String>;
}

/// What the repository holds that verified, and what it refused.
#[derive(Debug, Default)]
pub struct Checked {
    /// Each package's index entry and bytes, in the order its role lists them.
    pub packages: Vec<(Entry, Vec<u8>)>,
    /// Each refused target's path, and why.
    pub refused: Vec<(String, String)>,
}

struct Metadata<'a>(&'a mut dyn Files);

impl Remote for Metadata<'_> {
    fn fetch(&mut self, name: &str, max: u64) -> Result<Option<Vec<u8>>, kaituf::Error> {
        self.0
            .file(&format!("{METADATA}/{name}"), max)
            .map_err(kaituf::Error::Io)
    }
}

#[derive(Default)]
struct Memory(BTreeMap<String, Vec<u8>>);

impl Cache for Memory {
    fn load(&mut self, role: &str) -> Option<Vec<u8>> {
        self.0.get(role).cloned()
    }

    fn save(&mut self, role: &str, bytes: &[u8]) -> Result<(), kaituf::Error> {
        self.0.insert(role.into(), bytes.into());
        Ok(())
    }
}

/// The image repository in `files`, checked at `now` (seconds since 1970 UTC),
/// as a closure over `provided`, the OS's virtual packages: a package whose
/// dependencies the kept ones and `provided` do not meet is refused too, in
/// resolution's words. An error means the metadata did not verify, and nothing
/// in it is kept.
pub fn check(
    files: &mut dyn Files,
    now: u64,
    provided: &[(&str, Version)],
) -> Result<Checked, String> {
    let root = files
        .file(ROOT, updater::ROOT_MAX)?
        .ok_or_else(|| format!("the image has no {ROOT}"))?;
    let mut cache = Memory::default();
    cache.0.insert("root".into(), root);
    let mut listed = Vec::new();
    let mut checked = Checked::default();
    {
        let tuf = |e: kaituf::Error| e.to_string();
        let mut remote = Metadata(files);
        let mut updater = Updater::new(&mut remote, &mut cache, now).map_err(tuf)?;
        updater.refresh().map_err(tuf)?;
        for path in updater.listed(ROLE, "targets").map_err(tuf)? {
            match updater.target(&path) {
                Ok(Some(target)) => {
                    let file = updater.target_url(&target);
                    listed.push((target, file));
                }
                Ok(None) => checked
                    .refused
                    .push((path, "no trusted role names it".into())),
                Err(why) => checked.refused.push((path, why.to_string())),
            }
        }
    }
    for (target, file) in listed {
        let kept = |name: &str| checked.packages.iter().any(|(e, _)| e.name == name);
        match package(files, &target, &file) {
            Ok((entry, _)) if kept(&entry.name) => {
                let why = format!("the image has another version of {}", entry.name);
                checked.refused.push((target.path, why));
            }
            Ok(package) => checked.packages.push(package),
            Err(why) => checked.refused.push((target.path, why)),
        }
    }
    close(&mut checked, provided);
    Ok(checked)
}

/// Refuses, one at a time, each kept package that does not resolve against the
/// others and `provided`, until all do: generation 1 is a closure (§5).
fn close(checked: &mut Checked, provided: &[(&str, Version)]) {
    loop {
        let index: Vec<Entry> = checked.packages.iter().map(|(e, _)| e.clone()).collect();
        let unmet = index.iter().enumerate().find_map(|(at, entry)| {
            let want = [Want {
                name: &entry.name,
                range: None,
            }];
            resolve::resolve(&index, provided, &[], &want)
                .err()
                .map(|why| (at, why))
        });
        let Some((at, why)) = unmet else {
            return;
        };
        let (entry, _) = checked.packages.remove(at);
        let path = format!("{}-{}.kpkg", entry.name, entry.version);
        checked.refused.push((path, why));
    }
}

/// `target`'s bytes from the image, checked, and its index entry.
fn package(
    files: &mut dyn Files,
    target: &TargetFile,
    file: &str,
) -> Result<(Entry, Vec<u8>), String> {
    let bytes = files
        .file(&format!("{TARGETS}/{file}"), target.length)?
        .ok_or("it is not in the image")?;
    updater::check_target(target, &bytes).map_err(|e| e.to_string())?;
    Ok((entry(target, &bytes)?, bytes))
}

/// `target`'s index entry, from its `custom.kai` fields, each of which must be
/// what the package's own header says; `bytes` are the package, already checked
/// against `target`.
pub fn entry(target: &TargetFile, bytes: &[u8]) -> Result<Entry, String> {
    let files = bundle::files(bytes)?;
    let text = files
        .first()
        .filter(|(path, _)| *path == HEADER)
        .map(|(_, text)| *text)
        .ok_or("its first file is not kai-package")?;
    let text = core::str::from_utf8(text).map_err(|_| "its header is not text")?;
    let header = Header::parse(text)?;
    let kai = target
        .custom
        .get("kai")
        .and_then(Value::as_object)
        .ok_or("it has no custom.kai")?;
    if let Some(key) = kai.keys().find(|key| !FIELDS.contains(&key.as_str())) {
        return Err(format!("custom.kai has an unknown field {key}"));
    }
    let depends: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("depends "))
        .collect();
    let version = header.version.to_string();
    let same = [
        ("name", Some(header.name.as_str())),
        ("version", Some(version.as_str())),
        ("kind", Some(header.kind.name())),
        ("summary", Some(header.summary.as_str())),
        ("licence", Some(header.licence.as_str())),
        ("world", header.world.as_deref()),
    ];
    for (key, own) in same {
        let indexed = match kai.get(key) {
            None => None,
            Some(value) => Some(value.as_str().ok_or_else(|| not_a(key, "string"))?),
        };
        if indexed != own {
            return Err(format!("its header's {key} is not its index entry's"));
        }
    }
    if strings(kai, "depends")? != depends {
        return Err("its header's depends are not its index entry's".into());
    }
    let grants = strings(kai, "grants")?
        .into_iter()
        .map(String::from)
        .collect();
    strings(kai, "actions")?;
    let yanked = kai
        .get("yanked")
        .and_then(Value::as_bool)
        .ok_or_else(|| not_a("yanked", "boolean"))?;
    let path = format!("{}-{version}.kpkg", header.name);
    if target.path != path {
        return Err(format!("it is named {}, not {path}", target.path));
    }
    Ok(Entry {
        name: header.name,
        version: header.version,
        sha: sha256(target)?,
        size: target.length,
        depends: header.depends,
        grants,
        yanked,
    })
}

fn not_a(key: &str, what: &str) -> String {
    format!("custom.kai.{key} is not a {what}")
}

fn strings<'a>(kai: &'a Map<String, Value>, key: &str) -> Result<Vec<&'a str>, String> {
    let list = kai
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| not_a(key, "list"))?;
    list.iter()
        .map(|item| item.as_str().ok_or_else(|| not_a(key, "list of strings")))
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HEADER_TEXT: &str = "kai-package 1\nname hello\nversion 1.0.0\nkind data\nsummary A greeting\nlicence MIT\ndepends spell-en ^1.2\n";

    fn package() -> Vec<u8> {
        bundle::build(&[(HEADER, HEADER_TEXT.as_bytes()), ("hello.txt", b"hello\n")])
    }

    fn target(kai: Value, bytes: &[u8]) -> TargetFile {
        TargetFile {
            path: "hello-1.0.0.kpkg".into(),
            length: bytes.len() as u64,
            hashes: alloc::vec![("sha256".into(), crate::sha256(bytes).to_vec())],
            custom: json!({ "kai": kai }),
        }
    }

    fn index() -> Value {
        json!({
            "name": "hello", "version": "1.0.0", "kind": "data", "summary": "A greeting",
            "licence": "MIT", "depends": ["spell-en ^1.2"], "grants": ["files"],
            "actions": [], "yanked": false,
        })
    }

    #[test]
    fn an_index_entry_is_its_packages_own_header() {
        let bytes = package();
        let entry = entry(&target(index(), &bytes), &bytes).unwrap();
        assert_eq!(entry.name, "hello");
        assert_eq!(entry.version.to_string(), "1.0.0");
        assert_eq!(entry.sha, crate::sha256(&bytes));
        assert_eq!(entry.size, bytes.len() as u64);
        assert_eq!(entry.grants, ["files"]);
        assert_eq!(entry.depends.len(), 1);
    }

    #[test]
    fn an_entry_that_differs_from_its_header_is_refused() {
        let bytes = package();
        let with = |key: &str, value: Value| {
            let mut kai = index();
            kai[key] = value;
            entry(&target(kai, &bytes), &bytes).unwrap_err()
        };
        assert!(with("version", json!("1.0.1")).contains("version"));
        assert!(with("summary", json!("Run this")).contains("summary"));
        assert!(with("depends", json!([])).contains("depends"));
        assert!(with("world", json!("kai:app@0.1")).contains("world"));
        assert!(with("yanked", json!("no")).contains("boolean"));
        assert!(with("grants", json!([1])).contains("list of strings"));
        assert!(with("extra", json!(1)).contains("unknown field extra"));
        let mut renamed = target(index(), &bytes);
        renamed.path = "hello-2.0.0.kpkg".into();
        assert!(entry(&renamed, &bytes).unwrap_err().contains("named"));
        let mut missing = index();
        missing.as_object_mut().unwrap().remove("name");
        assert!(entry(&target(missing, &bytes), &bytes).is_err());
    }
}
