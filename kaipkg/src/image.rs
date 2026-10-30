//! The image's signed repository (ADR 0011 §3, §8; P4 task 1): the built-in
//! packages the slot image carries, checked with `kaituf`, become generation 1.
//!
//! The repository is `cargo xtask pkgrepo build`'s, as files by path:
//! `metadata/1.root.json` and the other roles beside it, and each package at
//! `targets/<name>-<version>.kpkg`, its target path. A mirror names targets by
//! their SHA-256 as well (consistent snapshots), but `kai-bundle 1` paths are at
//! most 64 bytes, too short for that; the hash is checked from the signed
//! metadata either way. The image's own `1.root.json` is the root it is checked
//! against: the image carries its root as it carries the kernel, and a `dev`
//! image's is a throwaway test root (P4 plan §4, item 3).
//!
//! Metadata that does not verify refuses the whole repository. Each target the
//! [`ROLE`] role lists is then checked on its own: its length and SHA-256, then
//! its own header against its `custom.kai` index entry (cargo's rule). One that
//! fails is refused by name and left out; the rest are kept.

use crate::bundle::{self, HEADER};
use crate::header::Header;
use crate::index::Index;
use crate::resolve::{self, Entry, Want};
use crate::version::Version;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use kaituf::meta::TargetFile;
use kaituf::updater::{self, Cache, Remote, Updater};

/// The role that lists every package (ADR 0011 §3), delegated by `targets`.
pub const ROLE: &str = "packages";
pub const METADATA: &str = "metadata";
pub const TARGETS: &str = "targets";
/// The root the image carries.
pub const ROOT: &str = "metadata/1.root.json";

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
    /// The index entry of each kept package, for the catalogue
    /// (`crate::catalog`).
    pub indexes: Vec<Index>,
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
    fn load(&mut self, role: &str) -> Result<Option<Vec<u8>>, kaituf::Error> {
        Ok(self.0.get(role).cloned())
    }

    fn save(&mut self, role: &str, bytes: &[u8]) -> Result<(), kaituf::Error> {
        self.0.insert(role.into(), bytes.into());
        Ok(())
    }
}

/// The time the image repository is checked at: before every expiry date. It
/// comes from the boot medium with the kernel, so it is as trusted as the image
/// is, and checked against the date an image would refuse its own packages a
/// week after it was built, once its timestamp ran out.
const UNEXPIRED: u64 = 0;

/// The image repository in `files`, its expiry dates not checked
/// ([`UNEXPIRED`]), as a closure over `provided`, the OS's virtual packages: a
/// package whose dependencies the kept ones and `provided` do not meet is
/// refused too, in resolution's words. An error means the metadata did not
/// verify, and nothing in it is kept.
pub fn check(files: &mut dyn Files, provided: &[(&str, Version)]) -> Result<Checked, String> {
    let Listed { targets, refused } = listed(files, ROLE)?;
    let mut checked = Checked {
        refused,
        ..Checked::default()
    };
    for target in targets {
        let kept = |name: &str| checked.packages.iter().any(|(e, _)| e.name == name);
        match package(files, &target) {
            Ok((entry, ..)) if kept(&entry.name) => {
                let why = format!("the image has another version of {}", entry.name);
                checked.refused.push((target.path, why));
            }
            Ok((entry, bytes, index)) => {
                checked.packages.push((entry, bytes));
                checked.indexes.push(index);
            }
            Err(why) => checked.refused.push((target.path, why)),
        }
    }
    close(&mut checked, provided);
    let kept = |i: &Index| {
        let same = |e: &Entry| e.name == i.name && e.version == i.version;
        checked.packages.iter().any(|(e, _)| same(e))
    };
    let indexes = core::mem::take(&mut checked.indexes);
    checked.indexes = indexes.into_iter().filter(|i| kept(i)).collect();
    Ok(checked)
}

/// What one role lists, from [`listed`].
pub struct Listed {
    /// Each target, as the search of the delegations finds it.
    pub targets: Vec<TargetFile>,
    /// Each listed path no trusted role names, and why.
    pub refused: Vec<(String, String)>,
}

/// The targets `role`, as `targets` delegates it, lists in the image's
/// repository ([`UNEXPIRED`]). An error means the metadata did not verify.
pub fn listed(files: &mut dyn Files, role: &str) -> Result<Listed, String> {
    let root = files
        .file(ROOT, updater::ROOT_MAX)?
        .ok_or_else(|| format!("the image has no {ROOT}"))?;
    let mut cache = Memory::default();
    cache.0.insert("root".into(), root);
    let tuf = |e: kaituf::Error| e.to_string();
    let mut remote = Metadata(files);
    let mut updater = Updater::new(&mut remote, &mut cache, UNEXPIRED).map_err(tuf)?;
    updater.refresh().map_err(tuf)?;
    let (mut targets, mut refused) = (Vec::new(), Vec::new());
    for path in updater.listed(role, "targets").map_err(tuf)? {
        match updater.target(&path) {
            Ok(Some(target)) => targets.push(target),
            Ok(None) => refused.push((path, "no trusted role names it".into())),
            Err(why) => refused.push((path, why.to_string())),
        }
    }
    Ok(Listed { targets, refused })
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

/// `target`'s bytes from the image, at its target path, checked, and its index
/// entry.
fn package(files: &mut dyn Files, target: &TargetFile) -> Result<(Entry, Vec<u8>, Index), String> {
    let bytes = files
        .file(&format!("{TARGETS}/{}", target.path), target.length)?
        .ok_or("it is not in the image")?;
    updater::check_target(target, &bytes).map_err(|e| e.to_string())?;
    let (entry, index) = entry_and_index(target, &bytes)?;
    Ok((entry, bytes, index))
}

/// `target`'s index entry, each of whose `custom.kai` fields must be what the
/// package's own header says ([`Index::matches`]); `bytes` are the package,
/// already checked against `target`.
pub fn entry(target: &TargetFile, bytes: &[u8]) -> Result<Entry, String> {
    entry_and_index(target, bytes).map(|(entry, _)| entry)
}

fn entry_and_index(target: &TargetFile, bytes: &[u8]) -> Result<(Entry, Index), String> {
    let index = Index::parse(target)?;
    let files = bundle::files(bytes)?;
    let text = files
        .first()
        .filter(|(path, _)| *path == HEADER)
        .map(|(_, text)| *text)
        .ok_or("its first file is not kai-package")?;
    let text = core::str::from_utf8(text).map_err(|_| "its header is not text")?;
    index.matches(&Header::parse(text)?, text)?;
    index.check_path(target)?;
    Ok((index.entry()?, index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

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
