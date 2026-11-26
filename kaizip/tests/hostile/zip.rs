//! The `zip_open` and `zip_extract` fuzz targets' bodies (plan P6 G27), which
//! `kaizip/tests/hostile.rs` also runs over hostile archives: a later
//! `fuzz/fuzz_targets/{zip_open,zip_extract}.rs` includes this file by
//! `#[path]` and calls [`zip_open`] or [`zip_extract`] with libFuzzer's input.
//! They must return for any input, and only a bug in kaizip may panic.
//!
//! The promises, under [`LIMITS`] (small, so a bomb shows within one run):
//!
//! - **The ceilings hold.** An archive that opens lists at most
//!   `max_entries` entries, each at most `max_entry` and all together at most
//!   `max_total`; extraction writes no file past the size it listed, and no
//!   more than `max_total` in all. `tests/hostile.rs` also bounds what the
//!   bodies allocate at once.
//! - **No path escapes the extract root.** Every folder and file lands under
//!   [`ROOT`], each part a plain name, each folder made before anything in
//!   it and nothing made twice.
//!
//! `fuzz/zip_harness.rs`'s `zip_archive` (the target on main) checks what
//! extraction gives in memory; these bodies add the root and the bounds.

#![allow(dead_code)]

use kaizip::{Archive, Entry, Kind, Limits, Unpack, names};
use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};

/// Small limits, so a bomb shows within one run (as `fuzz/zip_harness.rs`).
pub const LIMITS: Limits = Limits {
    max_entries: 256,
    max_entry: 1 << 20,
    max_total: 4 << 20,
    max_ratio: 250,
    ratio_floor: 64 * 1024,
    max_depth: 8,
};

/// Where [`zip_extract`] extracts to, as Files would to a folder.
pub const ROOT: &str = "/user/files/Downloads/extracted";

/// Opens `data` as an archive; what it lists keeps every ceiling, and each
/// path is plain and named once.
pub fn zip_open(data: &[u8]) -> Option<Archive<Cursor<&[u8]>>> {
    let archive = Archive::open(Cursor::new(data), LIMITS).ok()?;
    listed_holds(archive.entries());
    Some(archive)
}

/// [`zip_open`], then extracts what opened under [`ROOT`].
pub fn zip_extract(data: &[u8]) {
    let Some(mut archive) = zip_open(data) else {
        return;
    };
    let entries = archive.entries().to_vec();
    let mut out = Rooted::default();
    let done = archive.extract(&mut out);
    assert!(out.open.is_none() || done.is_err(), "a file left open");
    if done.is_ok() {
        // Every entry listed was made: a file at its size.
        for entry in &entries {
            let path = Path::new(ROOT).join(&entry.path);
            match entry.kind {
                Kind::File => assert_eq!(out.files.get(&path), Some(&entry.size)),
                Kind::Folder => assert!(out.folders.contains(&path)),
            }
        }
    }
}

fn listed_holds(entries: &[Entry]) {
    assert!(entries.len() <= LIMITS.max_entries);
    let mut total = 0u64;
    let mut paths = BTreeSet::new();
    for entry in entries {
        let (path, slash) = names::check(&entry.path, &LIMITS).expect("a listed path is plain");
        assert!(path == entry.path && !slash, "{:?}", entry.path);
        assert!(entry.size <= LIMITS.max_entry, "{entry:?}");
        total += entry.size;
        assert!(
            paths.insert(entry.path.clone()),
            "{} listed twice",
            entry.path
        );
    }
    assert!(total <= LIMITS.max_total, "{total} bytes listed");
    // No file has anything under it.
    for entry in entries.iter().filter(|e| e.kind == Kind::File) {
        let under = format!("{}/", entry.path);
        assert!(
            !entries.iter().any(|e| e.path.starts_with(&under)),
            "{}",
            entry.path
        );
    }
}

/// An extraction under [`ROOT`] that keeps no bytes, only what it was told,
/// and checks every call against the promises.
#[derive(Default)]
struct Rooted {
    folders: BTreeSet<PathBuf>,
    /// Each file's bytes written.
    files: std::collections::BTreeMap<PathBuf, u64>,
    /// The open file, its size as listed and its bytes so far.
    open: Option<(PathBuf, u64, u64)>,
    total: u64,
}

impl Rooted {
    /// `path` under the root, checked: plain parts only, a new name, and its
    /// folder made already.
    fn place(&self, path: &str) -> PathBuf {
        let root = Path::new(ROOT);
        let at = root.join(path);
        let rest = at.strip_prefix(root).expect("a path under the root");
        assert!(!rest.as_os_str().is_empty(), "the root itself, {path:?}");
        for part in rest.components() {
            let Component::Normal(part) = part else {
                panic!("{path:?} has a part {part:?} that is not a plain name");
            };
            let part = part.to_str().expect("a part is text");
            assert!(!part.contains(['\\', ':', '\0']), "{path:?}");
        }
        assert!(
            !self.folders.contains(&at) && !self.files.contains_key(&at),
            "{path:?} made twice"
        );
        let parent = at.parent().expect("a path under the root has a parent");
        assert!(
            parent == root || self.folders.contains(parent),
            "{path:?} before its folder"
        );
        at
    }
}

impl Unpack for Rooted {
    fn folder(&mut self, path: &str) -> Result<(), String> {
        assert!(self.open.is_none(), "a folder made with a file open");
        let at = self.place(path);
        self.folders.insert(at);
        Ok(())
    }

    fn begin(&mut self, path: &str, size: u64) -> Result<(), String> {
        assert!(self.open.is_none(), "a file begun with another open");
        assert!(size <= LIMITS.max_entry, "{path:?} at {size} bytes");
        let at = self.place(path);
        self.open = Some((at, size, 0));
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        let (path, size, written) = self.open.as_mut().expect("bytes with no file open");
        *written += bytes.len() as u64;
        self.total += bytes.len() as u64;
        assert!(*written <= *size, "{path:?} past its {size} bytes");
        assert!(self.total <= LIMITS.max_total, "past the total");
        Ok(())
    }

    fn end(&mut self) -> Result<(), String> {
        let (path, size, written) = self.open.take().expect("an end with no file open");
        assert_eq!(written, size, "{path:?} ended short");
        self.files.insert(path, written);
        Ok(())
    }
}
