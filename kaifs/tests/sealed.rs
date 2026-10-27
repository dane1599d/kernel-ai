//! Sealed files (ADR 0004 §8; P2 task 10): a model blob written in 2 MiB SEALED
//! records reads back as any file, its extents hold exactly its bytes for
//! `MODEL_FILL`, it is never rewritten, and a crash while it is written leaves
//! it whole or not there.

use kaifs::check::{Cache, check};
use kaifs::crash::{Options, Oracle, Recording, Rng, enumerate};
use kaifs::fs::MODELS;
use kaifs::ns_check::check_namespace;
use kaifs::{BLOCK, BlockDev, Error, Fs, Image, MemDisk, ROOT, RecordingDisk, format};

const BLOCKS: u64 = 8192;
const MIB2: u64 = 2 << 20;

/// The blob's bytes, by a generator of its own.
fn blob(len: u64) -> Vec<u8> {
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// Writes `data` as the sealed file `name` in `models`' root, a piece at a time.
fn seal<D: BlockDev>(fs: &mut Fs<D>, name: &str, data: &[u8]) -> Result<u64, Error> {
    fs.set_volume(MODELS)?;
    let mut at = 0;
    fs.write_sealed(ROOT, name, data.len() as u64, &mut |piece| {
        piece.copy_from_slice(&data[at..at + piece.len()]);
        at += piece.len();
        Ok(())
    })
}

/// What `MODEL_FILL` reads: the extents' blocks, in order, cut to the length.
fn through_extents(fs: &mut Fs<MemDisk>, ino: u64) -> Vec<u8> {
    let mut out = Vec::new();
    for (start, blocks) in fs.sealed_extents(ino).unwrap() {
        for addr in start..start + blocks {
            let mut block = [0; BLOCK];
            fs.io.dev.read(addr, &mut block).unwrap();
            out.extend_from_slice(&block);
        }
    }
    out
}

fn checks<D: BlockDev>(fs: &mut Fs<D>) -> Result<(), String> {
    let mut cache = Cache::default();
    check(fs, &mut cache)?;
    check_namespace(fs, &mut cache).map(|_| ())
}

#[test]
fn a_sealed_file_reads_back_and_its_extents_hold_it() {
    let mut fs = Fs::mkfs(MemDisk::new(BLOCKS), [4; 16], "sealed").unwrap();
    let data = blob(2 * MIB2 + 300_123);
    let ino = seal(&mut fs, "m.gguf", &data).unwrap();
    fs.commit().unwrap();
    for _ in 0..2 {
        fs.set_volume(MODELS).unwrap();
        assert!(fs.is_sealed(ino).unwrap());
        assert_eq!(fs.stat(ino).unwrap().size, data.len() as u64);
        assert_eq!(fs.read_file(ino).unwrap(), data);
        let at = MIB2 - 10;
        let across = fs.read_at(ino, at, 40).unwrap();
        assert_eq!(across, data[at as usize..at as usize + 40]);
        let held = through_extents(&mut fs, ino);
        assert_eq!(held[..data.len()], data[..]);
        assert!(
            held[data.len()..].iter().all(|&b| b == 0),
            "zeros after the tail"
        );
        let blocks: u64 = fs.sealed_extents(ino).unwrap().iter().map(|r| r.1).sum();
        assert_eq!(blocks, 512 + 512 + 300_123u64.div_ceil(BLOCK as u64));
        // A fresh disk's free space is one extent: the records lie in one run.
        assert_eq!(fs.sealed_extents(ino).unwrap().len(), 1);
        checks(&mut fs).unwrap();
        fs = Fs::mount(fs.into_dev()).unwrap();
    }
}

#[test]
fn a_sealed_file_is_never_rewritten_but_moves_and_goes_as_any_file() {
    let mut fs = Fs::mkfs(MemDisk::new(BLOCKS), [4; 16], "sealed").unwrap();
    let data = blob(MIB2 + 5);
    let ino = seal(&mut fs, "m.gguf", &data).unwrap();
    fs.commit().unwrap();
    let refused = Err(Error::Invalid("a sealed file is never rewritten"));
    assert_eq!(fs.write_at(ino, 0, b"x"), refused);
    assert_eq!(fs.truncate(ino, 1), refused);
    assert_eq!(fs.write_file(ino, b"y"), refused);
    fs.set_xattr(ino, "kai.fnv1a64", &7u64.to_le_bytes())
        .unwrap();
    let cache = fs.mkdir(ROOT, "cache").unwrap();
    fs.rename(ROOT, "m.gguf", cache, "m.gguf").unwrap();
    fs.commit().unwrap();
    assert_eq!(fs.read_file(ino).unwrap(), data);
    checks(&mut fs).unwrap();
    let (free, _) = fs.space();
    fs.remove(cache, "m.gguf").unwrap();
    fs.commit().unwrap();
    fs.commit().unwrap();
    fs.commit().unwrap();
    assert!(fs.space().0 >= free + 512, "its records are free again");
    checks(&mut fs).unwrap();
}

#[test]
fn a_small_blob_is_one_record_and_an_empty_one_is_not_sealed() {
    let mut fs = Fs::mkfs(MemDisk::new(BLOCKS), [4; 16], "sealed").unwrap();
    let small = seal(&mut fs, "small", b"ten bytes!").unwrap();
    let empty = seal(&mut fs, "empty", b"").unwrap();
    fs.commit().unwrap();
    assert!(fs.is_sealed(small).unwrap());
    assert_eq!(fs.read_file(small).unwrap(), b"ten bytes!");
    assert_eq!(fs.sealed_extents(small).unwrap().len(), 1);
    assert_eq!(fs.sealed_extents(small).unwrap()[0].1, 1);
    assert!(!fs.is_sealed(empty).unwrap());
    assert!(fs.sealed_extents(empty).is_err());
    assert_eq!(fs.read_file(empty).unwrap(), b"");
    checks(&mut fs).unwrap();
}

/// A crash state must mount, check, and hold the blob whole once its commit is
/// durable, and whole or not at all before.
struct Whole<'a> {
    data: &'a [u8],
    landed: u64,
}

impl Oracle for Whole<'_> {
    fn judge(&mut self, image: &mut Image<'_>, min: u64) -> Result<(), String> {
        let mut fs = Fs::mount(&mut *image).map_err(|e| format!("mount: {e:?}"))?;
        if fs.committed < min {
            return Err(format!("generation {}, older than {min}", fs.committed));
        }
        checks(&mut fs)?;
        fs.set_volume(MODELS).map_err(|e| format!("{e:?}"))?;
        match fs.lookup(ROOT, "m.gguf").map_err(|e| format!("{e:?}"))? {
            None if fs.committed >= self.landed => Err("the blob is gone".into()),
            None => Ok(()),
            Some(e) => match fs.read_file(e.ino) {
                Ok(bytes) if bytes == self.data && fs.is_sealed(e.ino) == Ok(true) => Ok(()),
                other => Err(format!("the blob reads {:?}", other.map(|b| b.len()))),
            },
        }
    }
}

#[test]
fn a_crash_while_a_blob_is_written_leaves_it_whole_or_not_there() {
    let mut fs = Fs::mkfs(RecordingDisk::new(4096), [4; 16], "sealed").unwrap();
    fs.io.dev.checkpoint();
    let base = fs.io.dev.disk.clone();
    let mut waited = vec![(0, fs.committed)];
    let data = blob(2 * MIB2 + 8192);
    seal(&mut fs, "m.gguf", &data).unwrap();
    let landed = fs.commit().unwrap();
    waited.push((fs.io.dev.log.len(), landed));
    let slots: Vec<u64> = format::all_slots(4096).collect();
    let tear = |addr: u64| slots.contains(&addr);
    let recording = Recording {
        base: &base,
        log: &fs.io.dev.log,
        waited: &waited,
        tear: &tear,
    };
    let options = match cfg!(debug_assertions) {
        true => Options {
            exhaustive: 12,
            random_subsets: 256,
            torn_subsets: 16,
        },
        false => Options::default(),
    };
    let mut oracle = Whole {
        data: &data,
        landed,
    };
    let tally = enumerate(&recording, &options, &mut oracle, &mut Rng(5));
    eprintln!(
        "sealed crash: {} states, {} violations",
        tally.states, tally.violations
    );
    assert_eq!(tally.violations, 0, "{:?}", tally.examples.first());
    assert!(tally.states > 100);
}
