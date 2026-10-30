//! An import from a disk, as `updated` streams it: the target its size names,
//! the hashes on the way, and each refusal.

use comp_models::{Refusal, Sha256, Signed};
use kabi::model::Fnv1a;
use modelchan::import::{Imported, start};
use sha2::{Digest, Sha256 as Hasher};

/// A GGUF file of `len` bytes: the magic, then filler.
fn gguf(len: usize) -> Vec<u8> {
    let mut bytes = b"GGUF\x03\0\0\0".to_vec();
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    while bytes.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        bytes.push(x as u8);
    }
    bytes.truncate(len);
    bytes
}

fn sha(bytes: &[u8]) -> Sha256 {
    Hasher::digest(bytes).into()
}

fn signed(id: &str, bytes: &[u8]) -> Signed {
    Signed {
        id: id.into(),
        name: format!("{id} model"),
        length: bytes.len() as u64,
        sha256: sha(bytes),
        licence: "MIT".into(),
        source: "generated".into(),
    }
}

/// The file as a disk: rounded up to whole sectors with zeros.
fn disk(bytes: &[u8]) -> Vec<u8> {
    let mut disk = bytes.to_vec();
    disk.resize(bytes.len().next_multiple_of(512), 0);
    disk
}

/// Streams `disk` in `chunk`-byte reads, as `updated` reads 128 KiB sectors.
fn import(role: &[Signed], disk: &[u8], chunk: usize) -> Result<Imported, Refusal> {
    let mut importing = start(role, disk.len() as u64)?;
    let (mut at, mut blob) = (0, Vec::new());
    while importing.read() < importing.disk() {
        let end = (at + chunk).min(disk.len());
        blob.extend_from_slice(importing.push(&disk[at..end])?);
        at = end;
    }
    assert_eq!(blob.len() as u64, importing.length());
    assert_eq!(
        &blob[..],
        &disk[..blob.len()],
        "the blob is the file's bytes"
    );
    importing.finish(role)
}

#[test]
fn a_signed_model_imports_with_both_hashes() {
    let model = gguf(4100);
    let role = [signed("small", &gguf(9000)), signed("fixture", &model)];
    for chunk in [1, 3, 512, 4096, 1 << 17] {
        let got = import(&role, &disk(&model), chunk).unwrap();
        let mut fnv = Fnv1a::new();
        fnv.update(&model);
        assert_eq!(
            got,
            Imported {
                id: "fixture".into(),
                length: 4100,
                sha256: sha(&model),
                fnv: fnv.finish(),
            },
            "{chunk}-byte reads"
        );
    }
    let importing = start(&role, 4608).unwrap();
    assert_eq!((importing.id(), importing.length()), ("fixture", 4100));
}

#[test]
fn a_disk_is_refused_before_it_is_read_unless_one_taken_target_fits() {
    let model = gguf(4100);
    let role = [signed("fixture", &model)];
    let refused = |role: &[Signed], disk: u64| start(role, disk).err().unwrap();
    assert_eq!(refused(&role, 5120), Refusal::NotSigned);
    assert_eq!(refused(&role, 4096), Refusal::NotSigned);
    let mut gpl = role.clone();
    gpl[0].licence = "GPL-3.0-only".into();
    assert_eq!(refused(&gpl, 4608), Refusal::Licence("GPL-3.0-only".into()));
    let twin = signed("twin", &gguf(4200));
    let both = [role[0].clone(), twin];
    assert_eq!(
        refused(&both, 4608),
        Refusal::TwoSizes("fixture".into(), "twin".into())
    );
}

#[test]
fn bytes_that_are_not_the_signed_ones_are_refused() {
    let model = gguf(4100);
    let role = [signed("fixture", &model)];
    let mut flipped = disk(&model);
    flipped[3000] ^= 1;
    assert_eq!(
        import(&role, &flipped, 512),
        Err(Refusal::Hash {
            id: "fixture".into()
        })
    );
    // Not a GGUF file: refused as soon as the magic is in, even byte by byte.
    let mut other = disk(&model);
    other[3] = b'X';
    for chunk in [1, 2, 4096] {
        assert_eq!(import(&role, &other, chunk), Err(Refusal::NotGguf));
    }
    // A read that stopped early cannot give the signed hash.
    let mut short = start(&role, 4608).unwrap();
    short.push(&model[..2000]).unwrap();
    assert_eq!(short.finish(&role), Err(Refusal::Short));
    // Every byte of the file but none of the disk's last sector is short too.
    let mut partial = start(&role, 4608).unwrap();
    partial.push(&model).unwrap();
    assert_eq!(partial.finish(&role), Err(Refusal::Short));
    let empty = start(&role, 4608).unwrap();
    assert_eq!(empty.finish(&role), Err(Refusal::Short));
}

#[test]
fn the_sectors_past_the_file_must_be_zeros_and_are_never_hashed() {
    let model = gguf(4100);
    let role = [signed("fixture", &model)];
    let mut padded = disk(&model);
    assert_eq!(import(&role, &padded, 4096).unwrap().sha256, sha(&model));
    padded[4607] = 1;
    for chunk in [1, 100, 4096, 8192] {
        assert_eq!(import(&role, &padded, chunk), Err(Refusal::Trailing));
    }
    // Bytes past the disk's size are not taken.
    let mut importing = start(&role, 4608).unwrap();
    let mut longer = disk(&model);
    longer.extend_from_slice(&[0xff; 600]);
    assert_eq!(importing.push(&longer).unwrap(), &model[..]);
    assert_eq!(importing.read(), 4608);
    assert_eq!(importing.finish(&role).unwrap().length, 4100);
}
