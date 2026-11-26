//! The `zip_archive` fuzz target's body (plan P6 M-7's fuzz gate), which
//! `kaizip/tests/fuzz_harness.rs` also runs over hostile mutations: any
//! bytes are opened as an archive and, if they pass, extracted in memory.
//! It must return for any input, and what it gives must keep kaizip's
//! promises: every path plain, every file its listed size, nothing past the
//! limits.

#![allow(dead_code)]

use kaizip::{Archive, Kind, Limits, Tree, names};
use std::io::Cursor;

/// Small limits, so a bomb shows within one run.
const LIMITS: Limits = Limits {
    max_entries: 256,
    max_entry: 1 << 20,
    max_total: 4 << 20,
    max_ratio: 250,
    ratio_floor: 64 * 1024,
    max_depth: 8,
};

pub fn zip_archive(data: &[u8]) {
    let Ok(mut archive) = Archive::open(Cursor::new(data), LIMITS) else {
        return;
    };
    let entries = archive.entries().to_vec();
    assert!(entries.len() <= LIMITS.max_entries);
    for entry in &entries {
        // Each path is plain: checking it again gives it back as it is.
        let (path, slash) = names::check(&entry.path, &LIMITS).expect("a listed path is plain");
        assert!(path == entry.path && !slash, "{:?}", entry.path);
        assert!(entry.size <= LIMITS.max_entry);
    }
    let mut tree = Tree::default();
    if archive.extract(&mut tree).is_err() {
        return;
    }
    let total: usize = tree.files.values().map(Vec::len).sum();
    assert!(total as u64 <= LIMITS.max_total, "past the total");
    for entry in entries.iter().filter(|e| e.kind == Kind::File) {
        assert_eq!(
            tree.files[&entry.path].len() as u64,
            entry.size,
            "{}",
            entry.path
        );
    }
}
