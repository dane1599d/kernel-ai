//! Hostile `kai-bundle 1` files (plan P6 G31): random bytes, every prefix,
//! every header and table field at every extreme, and random mutations, each
//! checked as it is and with its digests rewritten. Each parses into a bundle
//! that keeps every promise the boot files rely on (`hostile/harness.rs`), or
//! fails with an error. None panics.

#[path = "hostile/harness.rs"]
mod harness;
#[path = "../../testkit/hostile.rs"]
mod hostile;

use hostile::{XorShift, fuzz, sweep_fields, truncations};
use kaibundle::{ALIGN, ENTRY_BYTES, HEADER_BYTES, MAGIC, MAX_PATH, VERSION, parse, sha256};

/// A `kai-bundle 1` of `files`, written here rather than by `build` (feature
/// `alloc`), so the test needs no feature.
fn bundle(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    let mut offset = (HEADER_BYTES + files.len() * ENTRY_BYTES).next_multiple_of(ALIGN);
    for (path, data) in files {
        let mut field = [0u8; MAX_PATH];
        field[..path.len()].copy_from_slice(path.as_bytes());
        out.extend_from_slice(&field);
        out.extend_from_slice(&(offset as u64).to_le_bytes());
        out.extend_from_slice(&(data.len() as u64).to_le_bytes());
        out.extend_from_slice(&sha256::digest(data));
        offset = (offset + data.len()).next_multiple_of(ALIGN);
    }
    for (_, data) in files {
        out.resize(out.len().next_multiple_of(ALIGN), 0);
        out.extend_from_slice(data);
    }
    out
}

/// Small files, so each case stays a few pages.
fn sample() -> Vec<u8> {
    bundle(&[
        ("kernel.lines", b"lines"),
        ("user/shell.elf", &[0x7f, b'E', b'L', b'F', 1, 2, 3]),
        ("empty", b""),
    ])
}

const TABLE_END: usize = HEADER_BYTES + 3 * ENTRY_BYTES;

#[test]
fn the_sample_parses_and_keeps_its_promises() {
    let data = sample();
    assert_eq!(parse(&data).map(|b| b.len()), Ok(3));
    harness::kai_bundle(&data);
}

#[test]
fn random_bytes_never_panic() {
    let mut rng = XorShift::new(0xb0d1e);
    for _ in 0..5_000 {
        let len = rng.below(TABLE_END + 64);
        let mut bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        // Half behind a valid magic, version and small count, to reach the table.
        if len >= HEADER_BYTES && rng.below(2) == 0 {
            bytes[..8].copy_from_slice(MAGIC);
            bytes[8..10].copy_from_slice(&VERSION.to_le_bytes());
            bytes[10..12].copy_from_slice(&(rng.below(4) as u16).to_le_bytes());
        }
        harness::kai_bundle(&bytes);
        harness::kai_bundle_table(&bytes, rng.next() >> rng.below(64));
    }
}

#[test]
fn every_prefix_is_refused_until_the_last_file_ends() {
    let data = sample();
    truncations(&data, |cut, prefix| {
        harness::kai_bundle(prefix);
        assert_eq!(parse(prefix).is_ok(), cut == data.len(), "cut at {cut}");
    });
}

#[test]
fn every_header_and_table_field_at_every_extreme() {
    let data = sample();
    sweep_fields(&data, 0..TABLE_END, |_, mutated| {
        harness::kai_bundle(mutated);
        harness::kai_bundle_table(&mutated[..TABLE_END], mutated.len() as u64);
    });
}

#[test]
fn random_mutations_of_the_header_and_table() {
    // Mutations past the table only change file bytes, which the digest
    // catches; so mutate the table and put the files back.
    let data = sample();
    for seed in [7, 0x2545_f491_4f6c_dd1d] {
        fuzz(&data[..TABLE_END], seed, 3_000, |head| {
            let mut whole = head.to_vec();
            whole.extend_from_slice(data.get(head.len()..).unwrap_or(&[]));
            harness::kai_bundle(&whole);
        });
    }
}

/// Where table entry `i` starts.
const fn entry(i: usize) -> usize {
    HEADER_BYTES + i * ENTRY_BYTES
}

#[test]
fn rewritten_digests_still_refuse_a_bad_table() {
    let base = sample();
    type Edit = fn(&mut Vec<u8>);
    let edits: [(&str, Edit); 6] = [
        // The second entry's path made the first's.
        ("duplicate", |b| {
            let first = b[entry(0)..entry(0) + MAX_PATH].to_vec();
            b[entry(1)..entry(1) + MAX_PATH].copy_from_slice(&first);
        }),
        // Data inside the table, off a page, and past the end.
        ("in the table", |b| {
            b[entry(0) + MAX_PATH..entry(0) + MAX_PATH + 8].copy_from_slice(&0u64.to_le_bytes())
        }),
        ("off a page", |b| {
            let at = entry(0) + MAX_PATH;
            let off = u64::from_le_bytes(b[at..at + 8].try_into().unwrap()) + 8;
            b[at..at + 8].copy_from_slice(&off.to_le_bytes());
        }),
        ("past the end", |b| {
            let at = entry(1) + MAX_PATH + 8;
            b[at..at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        }),
        // A zero byte inside a path, and an empty one.
        ("zero inside", |b| b[entry(1) + 2] = 0),
        ("empty path", |b| b[entry(0)..entry(0) + MAX_PATH].fill(0)),
    ];
    for (what, edit) in edits {
        let mut bytes = base.clone();
        edit(&mut bytes);
        harness::redigest(&mut bytes);
        assert!(parse(&bytes).is_err(), "{what}");
        harness::kai_bundle(&bytes);
    }
}

/// Writes the `kai_bundle` target's seeds, one file each, to
/// `$FUZZ_SEED_DIR/kai_bundle/`: the sample bundle.
#[test]
#[ignore = "writes the fuzz corpus; run by the fuzz workflow"]
fn write_fuzz_seeds() {
    let dir = std::env::var("FUZZ_SEED_DIR").expect("FUZZ_SEED_DIR names the corpus folder");
    let folder = std::path::Path::new(&dir).join("kai_bundle");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("sample"), sample()).unwrap();
}
