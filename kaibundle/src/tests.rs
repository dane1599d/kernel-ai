//! A round trip, then hostile bundles in the `testkit/hostile.rs` pattern: every
//! field broken in turn, and random bytes, must give an error, never a panic.

use super::*;
use std::vec;
use std::vec::Vec;

fn sample() -> Vec<u8> {
    build(&[
        ("kernel.lines", b"lines"),
        ("user/shell.elf", &[0x7f, b'E', b'L', b'F', 1, 2, 3]),
        ("comp/notes/2.wasm", &[0u8; 5000]),
        ("empty", b""),
    ])
    .unwrap()
}

#[test]
fn a_bundle_round_trips_its_files_in_order() {
    let bytes = sample();
    let bundle = parse(&bytes).unwrap();
    let paths: Vec<&str> = bundle.entries().map(|e| e.path).collect();
    assert_eq!(
        paths,
        [
            "kernel.lines",
            "user/shell.elf",
            "comp/notes/2.wasm",
            "empty"
        ]
    );
    assert_eq!(bundle.get("kernel.lines"), Some(&b"lines"[..]));
    assert_eq!(bundle.get("comp/notes/2.wasm").map(<[u8]>::len), Some(5000));
    assert_eq!(bundle.get("empty"), Some(&b""[..]));
    assert_eq!(bundle.get("missing"), None);
    for e in bundle.entries() {
        let offset = e.data.as_ptr() as usize - bytes.as_ptr() as usize;
        assert!(offset.is_multiple_of(ALIGN), "{} at {offset}", e.path);
    }
}

#[test]
fn build_refuses_bad_paths() {
    assert_eq!(build(&[("", b"x")]), Err(Error::BadPath));
    assert_eq!(build(&[("a\0b", b"x")]), Err(Error::BadPath));
    let long = "p".repeat(MAX_PATH + 1);
    assert_eq!(build(&[(long.as_str(), b"x")]), Err(Error::BadPath));
    assert_eq!(
        build(&[("a", b"x"), ("a", b"y")]),
        Err(Error::DuplicatePath)
    );
    assert!(build(&[("p".repeat(MAX_PATH).as_str(), b"x")]).is_ok());
}

/// Offset of entry `i`'s field `at` (0 path, 64 offset, 72 length, 80 digest).
fn field(i: usize, at: usize) -> usize {
    HEADER_BYTES + i * ENTRY_BYTES + at
}

#[test]
fn every_broken_field_is_refused() {
    let good = sample();
    let broken = |change: &dyn Fn(&mut Vec<u8>)| {
        let mut bytes = good.clone();
        change(&mut bytes);
        parse(&bytes).err()
    };
    assert_eq!(broken(&|b| b.truncate(10)), Some(Error::Truncated));
    assert_eq!(broken(&|b| b[0] = b'X'), Some(Error::BadMagic));
    assert_eq!(broken(&|b| b[8] = 2), Some(Error::UnknownVersion(2)));
    assert_eq!(
        broken(&|b| b[10..12].copy_from_slice(&5000u16.to_le_bytes())),
        Some(Error::TooManyEntries)
    );
    assert_eq!(
        broken(&|b| b[10..12].copy_from_slice(&900u16.to_le_bytes())),
        Some(Error::Truncated)
    );
    assert_eq!(broken(&|b| b[field(1, 0)] = 0), Some(Error::BadPath));
    assert_eq!(broken(&|b| b[field(1, 0)] = 0xFF), Some(Error::BadPath));
    assert_eq!(
        broken(&|b| b[field(1, 40)] = b'x'),
        Some(Error::BadPath),
        "bytes after the zero"
    );
    assert_eq!(
        broken(&|b| b[field(1, 64)] ^= 1),
        Some(Error::BadRange),
        "off a boundary"
    );
    assert_eq!(
        broken(&|b| b[field(1, 64)..field(1, 72)].copy_from_slice(&0u64.to_le_bytes())),
        Some(Error::BadRange),
        "inside the table"
    );
    assert_eq!(
        broken(&|b| b[field(1, 72)..field(1, 80)].copy_from_slice(&u64::MAX.to_le_bytes())),
        Some(Error::BadRange)
    );
    assert_eq!(broken(&|b| b[field(1, 80)] ^= 1), Some(Error::Corrupt));
    let data = parse(&good).unwrap().entries().nth(1).unwrap().data;
    let at = data.as_ptr() as usize - good.as_ptr() as usize;
    assert_eq!(broken(&|b| b[at] ^= 1), Some(Error::Corrupt));
    assert_eq!(
        broken(&|b| {
            let first = b[field(0, 0)..field(0, 64)].to_vec();
            b[field(1, 0)..field(1, 64)].copy_from_slice(&first);
        }),
        Some(Error::DuplicatePath)
    );
}

#[test]
fn random_bytes_never_panic() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let good = sample();
    for round in 0..2000 {
        let mut bytes = if round % 2 == 0 {
            good.clone()
        } else {
            vec![0u8; (next() % 9000) as usize]
        };
        if round % 2 == 1 && bytes.len() >= 16 {
            bytes[..8].copy_from_slice(MAGIC);
            bytes[8..10].copy_from_slice(&VERSION.to_le_bytes());
        }
        for _ in 0..1 + next() % 8 {
            if !bytes.is_empty() {
                let at = (next() as usize) % bytes.len();
                bytes[at] = next() as u8;
            }
        }
        let _ = parse(&bytes).map(|b| b.entries().count());
    }
}
