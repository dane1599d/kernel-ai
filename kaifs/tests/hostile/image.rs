//! The `kaifs_image` fuzz target's body (plan P6 G29), which
//! `kaifs/tests/hostile.rs` also runs over hostile images: a later
//! `fuzz/fuzz_targets/kaifs_image.rs` includes this file by `#[path]` and calls
//! [`kaifs_image`] with libFuzzer's input. It must return for any input, and
//! only a bug in kaifs may panic. [`seeds`] gives the corpus to start from.
//!
//! The input's first byte picks how the rest becomes a [`BLOCKS`]-block disk:
//!
//! - **Raw** (bit 0 clear): the rest is the disk itself, block 0 first; what it
//!   leaves out reads as zeros. With bit 1 set, every superblock slot is given
//!   a good checksum again, so the field checks behind it run.
//! - **Edits** (bit 0 set): the rest is a list of edits to a small good image,
//!   each `[target, offset (u16 LE), len, len bytes]`. Target `0xFF` is not an
//!   edit but a raw tree value for `decode_value`; one with its top bit set
//!   names any block by address; any other names one of the blocks the
//!   newest commit reaches. Edited blocks are then signed again up to the
//!   superblock ([`map`]), so the edit reaches the parsers behind the
//!   checksums, unless bit 1 is set.
//!
//! Every reader then runs: `read_rings`, `read_checked` on each slot's roots,
//! the mount, `decode_value`, `check` and `check_namespace`, and a walk of
//! every volume. The promises:
//!
//! - **No allocation past the image's own size.** Nothing a reader returns
//!   is longer than the disk; `tests/hostile.rs` also bounds the peak.
//! - **A check never says a damaged image is fine.** When `check` and
//!   `check_namespace` pass, every folder lists, every name stats and every
//!   file reads; a file written then commits, mounts again, checks again,
//!   reads back, and leaves every other file as it was.

#![allow(dead_code)]

#[path = "map.rs"]
pub mod map;

use kaifs::check::{Cache, check};
use kaifs::csum::Csum;
use kaifs::format::{self, Superblock};
use kaifs::fs::{HOME, SYSTEM};
use kaifs::io::decode_value;
use kaifs::ns_check::check_namespace;
use kaifs::vol::SnapKind;
use kaifs::{BLOCK, Block, CsumKind, Error, FileType, Fs, MemDisk, ROOT, Value};
use map::{Map, block};
use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

/// Small, so a run is fast; big enough for two volumes' trees and a snapshot.
pub const BLOCKS: u64 = 128;
/// The disk's own size, which nothing read from it may pass.
pub const IMAGE_BYTES: usize = BLOCKS as usize * BLOCK;

/// Bit 0 of the first byte: edits to the sample image, not a raw disk.
pub const EDITS: u8 = 1;
/// Bit 1: raw, sign the superblock slots; edits, leave the edits unsigned.
pub const FLIP: u8 = 2;
/// The edit target that is a raw value for `decode_value` instead.
pub const VALUE: u8 = 0xFF;
/// Edits read from one input, so a run stays short.
const MAX_EDITS: usize = 64;
/// What the walk writes to a checked image.
const WRITTEN: &[u8] = b"written by the kaifs_image body after check passed";
const WRITTEN_NAME: &str = "kaifs-image-body";

/// A disk with two volumes' worth of history: folders, inline and record
/// data, attributes, actions, the trash and a named snapshot.
pub fn sample() -> MemDisk {
    let mut fs = Fs::mkfs(MemDisk::new(BLOCKS), [7; 16], "fuzz").unwrap();
    for n in 0..6u64 {
        fs.begin(n + 1).unwrap();
        fs.set_volume([HOME, SYSTEM][n as usize % 2]).unwrap();
        let dir = match fs.lookup(ROOT, "d").unwrap() {
            Some(e) => e.ino,
            None => fs.mkdir(ROOT, "d").unwrap(),
        };
        let f = fs.create(dir, &format!("f{n}")).unwrap();
        // Inline and one- and two-block records.
        fs.write_file(f, &vec![n as u8 + 1; [40, 3000, 6000][n as usize % 3]])
            .unwrap();
        if n % 3 == 0 {
            fs.set_xattr(f, "user.n", &[n as u8; 12]).unwrap();
        }
        if n == 4 {
            fs.trash(dir, "f2").unwrap();
        }
        fs.commit().unwrap();
    }
    fs.set_volume(HOME).unwrap();
    fs.snapshot_as(SnapKind::Named, 0, 0).unwrap();
    fs.commit().unwrap();
    fs.into_dev()
}

struct Base {
    disk: MemDisk,
    map: Map,
    addrs: Vec<u64>,
}

thread_local! {
    static BASE: OnceCell<Rc<Base>> = const { OnceCell::new() };
}

fn base() -> Rc<Base> {
    BASE.with(|cell| {
        cell.get_or_init(|| {
            let disk = sample();
            let map = Map::of(&mut disk.clone());
            let addrs = map.blocks.keys().copied().collect();
            Rc::new(Base { disk, map, addrs })
        })
        .clone()
    })
}

/// The corpus to start from: the sample as a raw disk, and as edits that
/// change nothing, signed and unsigned.
pub fn seeds() -> Vec<Vec<u8>> {
    let disk = base().disk.clone();
    let mut raw = vec![0];
    for addr in 0..BLOCKS {
        raw.extend_from_slice(&block(&disk, addr));
    }
    while raw.len() > 1 && raw[raw.len() - BLOCK..].iter().all(|&b| b == 0) {
        raw.truncate(raw.len() - BLOCK);
    }
    let no_op = [0u8, 0, 0, 0];
    vec![
        raw,
        [&[EDITS][..], &no_op].concat(),
        [&[EDITS | FLIP][..], &no_op].concat(),
    ]
}

pub fn kaifs_image(data: &[u8]) {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let (disk, values) = match mode & EDITS {
        0 => (raw(rest, mode & FLIP != 0), Vec::new()),
        _ => edited(rest, mode & FLIP == 0),
    };
    readers(disk, &values);
}

fn block_of(bytes: &[u8]) -> Block {
    let mut block = [0; BLOCK];
    block[..bytes.len().min(BLOCK)].copy_from_slice(&bytes[..bytes.len().min(BLOCK)]);
    block
}

/// Gives a superblock slot a good checksum again, whatever it holds.
fn resign(disk: &mut MemDisk, slot: u64) {
    let mut bytes = block(disk, slot);
    let sum = CsumKind::Seahash64.sum(&bytes[..BLOCK - 32]);
    bytes[BLOCK - 32..].copy_from_slice(&sum);
    disk.put(slot, Rc::new(bytes));
}

fn raw(bytes: &[u8], sign: bool) -> MemDisk {
    let mut disk = MemDisk::new(BLOCKS);
    for (addr, chunk) in bytes.chunks(BLOCK).take(BLOCKS as usize).enumerate() {
        disk.put(addr as u64, Rc::new(block_of(chunk)));
    }
    if sign {
        for slot in format::all_slots(BLOCKS) {
            resign(&mut disk, slot);
        }
    }
    disk
}

fn edited(mut rest: &[u8], sign: bool) -> (MemDisk, Vec<Vec<u8>>) {
    let base = base();
    let (mut disk, mut map) = (base.disk.clone(), base.map.clone());
    let slots: BTreeSet<u64> = format::all_slots(BLOCKS).collect();
    let (mut values, mut edits) = (Vec::new(), Vec::new());
    while let [target, lo, hi, len, tail @ ..] = rest {
        let n = usize::from(*len).min(tail.len());
        let bytes = &tail[..n];
        rest = &tail[n..];
        if *target == VALUE {
            values.push(bytes.to_vec());
            continue;
        }
        let addr = match target & 0x80 {
            0 => base.addrs[usize::from(*target) % base.addrs.len()],
            _ => u64::from(target & 0x7F) % BLOCKS,
        };
        edits.push((
            addr,
            usize::from(u16::from_le_bytes([*lo, *hi])) % BLOCK,
            bytes,
        ));
        if edits.len() + values.len() >= MAX_EDITS {
            break;
        }
    }
    let apply = |disk: &mut MemDisk, (addr, at, bytes): (u64, usize, &[u8])| {
        let mut changed = block(disk, addr);
        let n = bytes.len().min(BLOCK - at);
        changed[at..at + n].copy_from_slice(&bytes[..n]);
        disk.put(addr, Rc::new(changed));
    };
    // The tree first, signed; then the superblocks, which signing rewrites.
    let (sb_edits, tree_edits): (Vec<_>, Vec<_>) = edits
        .into_iter()
        .partition(|(addr, ..)| slots.contains(addr));
    for edit in tree_edits {
        apply(&mut disk, edit);
        if sign {
            map.sign(&mut disk, edit.0);
        }
    }
    for edit in sb_edits {
        apply(&mut disk, edit);
        if sign {
            resign(&mut disk, edit.0);
        }
    }
    (disk, values)
}

fn readers(mut disk: MemDisk, values: &[Vec<u8>]) {
    if let Ok(slots) = format::read_rings(&mut disk) {
        rings_hold(&mut disk, &slots);
    }
    let Ok(mut fs) = Fs::mount(disk) else {
        return;
    };
    for raw in values {
        if let Ok(value) = decode_value(raw, &mut fs.io) {
            let len = value.bytes().len();
            match value {
                Value::Inline(_) => assert_eq!(len + 1, raw.len()),
                Value::Record(_) => assert!(len <= IMAGE_BYTES, "a {len}-byte record"),
            }
        }
    }
    let mut cache = Cache::default();
    let fine = check(&mut fs, &mut cache).is_ok() && check_namespace(&mut fs, &mut cache).is_ok();
    let before = walk(&mut fs);
    if !fine {
        // A writer on a damaged image refuses or goes on; it must not panic.
        let _ = write(&mut fs);
        return;
    }
    let before = before.unwrap_or_else(|e| panic!("check passed, a reader refused: {e:?}"));
    let ino = match write(&mut fs) {
        Ok(Some(ino)) => ino,
        Ok(None) => return,
        Err(e) => panic!("check passed, the writer refused: {e:?}"),
    };
    let mut fs = Fs::mount(fs.into_dev()).expect("a checked image mounts after a commit");
    let mut cache = Cache::default();
    check(&mut fs, &mut cache).expect("a checked image checks after a commit");
    check_namespace(&mut fs, &mut cache).expect("and its namespace too");
    let mut after = walk(&mut fs).expect("a checked image reads after a commit");
    let (sum, _) = after.remove(&(HOME, ino)).expect("the file written");
    assert_eq!(sum, CsumKind::Seahash64.sum(WRITTEN));
    assert!(before == after, "the write changed another file");
}

/// The superblocks `read_rings` returns: of this disk, of one filesystem,
/// newest first, one per generation, and their roots read only in the data
/// area and only when their checksums hold.
fn rings_hold(disk: &mut MemDisk, slots: &[Superblock]) {
    let newest = slots
        .first()
        .expect("read_rings returns a superblock or refuses");
    let (start, end) = format::data_area(BLOCKS);
    for (i, sb) in slots.iter().enumerate() {
        assert_eq!(sb.block_count, BLOCKS);
        assert_eq!(sb.uuid, newest.uuid);
        assert!(i == 0 || sb.generation < slots[i - 1].generation);
        for ptr in [sb.volumes, sb.alloc, sb.gc, sb.keys] {
            if let Ok(read) = format::read_checked(disk, sb.csum_kind, &ptr, "root") {
                assert!(start <= ptr.addr && ptr.addr < end && ptr.blocks == 1);
                assert_eq!(sb.csum_kind.sum(&read[..]), ptr.csum);
            }
        }
    }
}

/// Every file's bytes, by their checksum (so the walk holds little of its
/// own), and attributes in every volume, by volume and inode; the first
/// refusal of any reader on the way.
type Files = BTreeMap<(u64, u64), (Csum, Vec<(String, Vec<u8>)>)>;

fn walk(fs: &mut Fs<MemDisk>) -> Result<Files, Error> {
    let mut files = Files::new();
    for (id, _) in fs.volumes() {
        fs.set_volume(id)?;
        let mut stack = vec![ROOT];
        let mut seen = BTreeSet::new();
        while let Some(dir) = stack.pop() {
            // A namespace that passed check is a tree, so this bounds only
            // the walk of one that did not.
            if !seen.insert(dir) || seen.len() > BLOCKS as usize * 64 {
                continue;
            }
            fs.xattrs(dir)?;
            for listed in fs.list(dir, None, usize::MAX)? {
                let ino = listed.entry.ino;
                match fs.stat(ino)?.kind {
                    FileType::Dir => stack.push(ino),
                    FileType::File => {
                        // Holes read as zeros, so a file may be longer than
                        // the disk: read no more than the disk holds.
                        let bytes = fs.read_at(ino, 0, IMAGE_BYTES)?;
                        let sum = CsumKind::Seahash64.sum(&bytes);
                        files.insert((id, ino), (sum, fs.xattrs(ino)?));
                    }
                }
            }
        }
    }
    Ok(files)
}

/// Writes [`WRITTEN`] to a new file in home's root, in one action: its inode,
/// or `None` when the disk is full, read-only or already has the name.
fn write(fs: &mut Fs<MemDisk>) -> Result<Option<u64>, Error> {
    let ok = |e: &Error| matches!(e, Error::NoSpace | Error::ReadOnly | Error::Exists);
    let done = (|| {
        fs.set_volume(HOME)?;
        fs.begin(1 << 40)?;
        let ino = fs.create(ROOT, WRITTEN_NAME)?;
        fs.write_file(ino, WRITTEN)?;
        fs.commit()?;
        Ok(ino)
    })();
    match done {
        Ok(ino) => Ok(Some(ino)),
        Err(e) if ok(&e) => Ok(None),
        Err(e) => Err(e),
    }
}
