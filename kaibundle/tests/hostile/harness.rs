//! The `kai_bundle` fuzz target's body (plan P6 G31), which
//! `kaibundle/tests/hostile.rs` also runs over hostile bundles, the way
//! `fuzz/harness.rs` serves its targets: a later
//! `fuzz/fuzz_targets/kai_bundle.rs` includes this file by `#[path]` and calls
//! [`kai_bundle`] with libFuzzer's input. It must return for any input, and
//! only a bug in `kaibundle` may panic.
//!
//! A bundle that parses keeps the promises the kernel's boot files rely on:
//! as many entries as its header says, each with a plain path given once and
//! data inside the bundle, past its table, on a 4 KiB boundary and matching
//! its digest; the streaming reader (`table_entry`) agrees entry by entry.

#![allow(dead_code)]

use kaibundle::{
    ALIGN, ENTRY_BYTES, HEADER_BYTES, MAX_ENTRIES, MAX_PATH, parse, sha256, table_bytes,
    table_entry,
};

/// Checks `data` as it is, then with every digest it can rewrite rewritten,
/// so a mutation is checked past the digest, not only refused by it.
pub fn kai_bundle(data: &[u8]) {
    check(data);
    let mut fixed = data.to_vec();
    if redigest(&mut fixed) {
        check(&fixed);
    }
}

/// Writes each entry's SHA-256 over its data, for every entry whose range lies
/// inside `bytes`; whether any was written.
pub fn redigest(bytes: &mut [u8]) -> bool {
    let Ok(size) = table_bytes(bytes) else {
        return false;
    };
    if bytes.len() < size {
        return false;
    }
    let mut any = false;
    for index in 0..(size - HEADER_BYTES) / ENTRY_BYTES {
        let at = HEADER_BYTES + index * ENTRY_BYTES + MAX_PATH;
        let word = |i: usize| u64::from_le_bytes(bytes[at + i..at + i + 8].try_into().unwrap());
        let (offset, length) = (word(0), word(8));
        let Some(range) = usize::try_from(offset)
            .ok()
            .zip(usize::try_from(length).ok())
            .and_then(|(start, len)| Some(start..start.checked_add(len)?))
            .filter(|r| r.end <= bytes.len())
        else {
            continue;
        };
        let digest = sha256::digest(&bytes[range]);
        bytes[at + 16..at + 48].copy_from_slice(&digest);
        any = true;
    }
    any
}

/// The promises, for `data` if it parses.
pub fn check(data: &[u8]) {
    let streamed = table_bytes(data);
    let Ok(bundle) = parse(data) else {
        return;
    };
    let size = streamed.expect("a parsed bundle has a header");
    let count = (size - HEADER_BYTES) / ENTRY_BYTES;
    assert!(count <= MAX_ENTRIES && size <= data.len());
    assert_eq!(bundle.len(), count);
    let base = data.as_ptr() as usize;
    let entries: Vec<_> = bundle.entries().collect();
    assert_eq!(entries.len(), count);
    for (index, entry) in entries.iter().enumerate() {
        assert!(!entry.path.is_empty() && entry.path.len() <= MAX_PATH);
        assert!(!entry.path.contains('\0'));
        let start = entry.data.as_ptr() as usize - base;
        assert!(
            start.is_multiple_of(ALIGN) && start >= size,
            "{}",
            entry.path
        );
        assert!(start + entry.data.len() <= data.len());
        let at = HEADER_BYTES + index * ENTRY_BYTES + MAX_PATH + 16;
        assert_eq!(sha256::digest(entry.data)[..], data[at..at + 32]);
        assert!(entries[..index].iter().all(|e| e.path != entry.path));
        assert_eq!(bundle.get(entry.path), Some(entry.data));
        // The streaming reader takes the same entry.
        let row =
            table_entry(&data[..size], index, data.len() as u64).expect("a parsed entry streams");
        assert_eq!(row.path, entry.path);
        assert_eq!(row.offset as usize, start);
        assert_eq!(row.length as usize, entry.data.len());
        assert_eq!(row.sha256[..], data[at..at + 32]);
    }
    assert!(table_entry(&data[..size], count, data.len() as u64).is_err());
}

/// The streaming reader alone, over any table and any claimed total: an entry
/// it gives lies inside the total, past the table, on a 4 KiB boundary.
pub fn kai_bundle_table(data: &[u8], total: u64) {
    let Ok(size) = table_bytes(data) else {
        return;
    };
    for index in 0..=MAX_ENTRIES {
        let Ok(row) = table_entry(data, index, total) else {
            continue;
        };
        assert!(index < (size - HEADER_BYTES) / ENTRY_BYTES);
        assert!(!row.path.is_empty() && row.path.len() <= MAX_PATH);
        assert!(row.offset.is_multiple_of(ALIGN as u64) && row.offset >= size as u64);
        assert!(
            row.offset
                .checked_add(row.length)
                .is_some_and(|e| e <= total)
        );
    }
}
