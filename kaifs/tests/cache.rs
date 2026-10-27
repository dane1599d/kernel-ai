//! The block cache (P2 task 8's): it changes nothing the device sees, so every
//! crash gate holds over a cached device; reads always see the last write; a
//! write the device refuses leaves nothing cached; and a tree read again is
//! served from memory.

use kaifs::cache::Cached;
use kaifs::check::{Cache, check};
use kaifs::crash::Rng;
use kaifs::fs::{HOME, SYSTEM};
use kaifs::ns_check::check_namespace;
use kaifs::{BLOCK, Block, BlockDev, DevInfo, Fs, IoError, MemDisk, Op, ROOT, RecordingDisk};

/// The same work on any device: three volumes, actions, a restore, a trash, a
/// GC pass and reads, so writes, re-reads and evictions all happen.
fn work<D: BlockDev>(fs: &mut Fs<D>) -> u64 {
    let mut rng = Rng(5);
    let mut files = Vec::new();
    for round in 0..30u64 {
        fs.begin(round + 1).unwrap();
        fs.set_volume([HOME, SYSTEM][round as usize % 2]).unwrap();
        let name = format!("f{}", rng.below(12));
        let ino = match fs.lookup(ROOT, &name).unwrap() {
            Some(e) => e.ino,
            None => fs.create(ROOT, &name).unwrap(),
        };
        let len = [10, 5_000, 140_000][rng.below(3) as usize];
        fs.write_file(ino, &vec![round as u8; len]).unwrap();
        files.push((fs.volume(), ino));
        fs.commit().unwrap();
        if round % 7 == 6 {
            let txn = fs.undo_records().unwrap().last().unwrap().txn;
            fs.begin(1_000 + round).unwrap();
            fs.restore(txn).unwrap();
            fs.commit().unwrap();
        }
    }
    fs.set_volume(HOME).unwrap();
    let first = fs.list(ROOT, None, 1).unwrap()[0].entry.name.clone();
    fs.begin(2_000).unwrap();
    fs.trash(ROOT, &first).unwrap();
    fs.commit().unwrap();
    fs.expire(u64::MAX).unwrap();
    fs.commit().unwrap();
    fs.gc(8).unwrap();
    fs.commit().unwrap();
    let mut sum = 0u64;
    for (vol, ino) in files {
        fs.set_volume(vol).unwrap();
        if let Ok(data) = fs.read_file(ino) {
            sum = sum
                .wrapping_mul(31)
                .wrapping_add(data.iter().map(|&b| u64::from(b)).sum());
        }
    }
    sum
}

fn same(a: &[Op], b: &[Op]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|pair| match pair {
            (Op::Write(x, bx), Op::Write(y, by)) => x == y && bx[..] == by[..],
            (Op::Flush, Op::Flush) => true,
            _ => false,
        })
}

#[test]
fn the_device_sees_the_same_writes_and_flushes_with_the_cache_or_without() {
    let mut bare = Fs::mkfs(RecordingDisk::new(8192), [3; 16], "cache").unwrap();
    let mut cached = Fs::mkfs(Cached::new(RecordingDisk::new(8192), 64), [3; 16], "cache").unwrap();
    let (a, b) = (work(&mut bare), work(&mut cached));
    assert_eq!(a, b, "what was read back differs");
    assert!(
        same(&bare.io.dev.log, &cached.io.dev.dev.log),
        "the device's logs differ"
    );
    let dev = &cached.io.dev;
    assert!(dev.hits > 0 && dev.misses > 0 && dev.len() == 64);
    for fs in [&mut bare as &mut dyn Checked, &mut cached] {
        fs.checked();
    }
}

trait Checked {
    fn checked(&mut self);
}

impl<D: BlockDev> Checked for Fs<D> {
    fn checked(&mut self) {
        let mut cache = Cache::default();
        check(self, &mut cache).unwrap();
        check_namespace(self, &mut cache).unwrap();
    }
}

#[test]
fn reads_always_see_the_last_write_whatever_was_evicted() {
    let mut cached = Cached::new(MemDisk::new(256), 8);
    let mut model = MemDisk::new(256);
    let mut rng = Rng(9);
    let mut buf: Block = [0; BLOCK];
    let mut want: Block = [0; BLOCK];
    for i in 0..20_000u64 {
        let addr = rng.below(48);
        match rng.below(3) {
            0 => {
                let mut block: Block = [0; BLOCK];
                block[..8].copy_from_slice(&i.to_le_bytes());
                block[BLOCK - 1] = addr as u8;
                cached.write(addr, &block).unwrap();
                model.write(addr, &block).unwrap();
            }
            _ => {
                cached.read(addr, &mut buf).unwrap();
                model.read(addr, &mut want).unwrap();
                assert_eq!(buf[..], want[..], "block {addr} at step {i}");
            }
        }
        assert!(cached.len() <= 8);
    }
}

/// A disk that refuses writes to one block.
struct Refusing {
    disk: MemDisk,
    bad: u64,
}

impl BlockDev for Refusing {
    fn info(&self) -> DevInfo {
        self.disk.info()
    }
    fn read(&mut self, addr: u64, buf: &mut Block) -> Result<(), IoError> {
        self.disk.read(addr, buf)
    }
    fn write(&mut self, addr: u64, data: &Block) -> Result<(), IoError> {
        match addr == self.bad {
            true => Err(IoError),
            false => self.disk.write(addr, data),
        }
    }
    fn flush(&mut self) -> Result<(), IoError> {
        self.disk.flush()
    }
}

#[test]
fn a_write_the_device_refuses_leaves_nothing_cached() {
    let mut cached = Cached::new(
        Refusing {
            disk: MemDisk::new(16),
            bad: 3,
        },
        4,
    );
    let old: Block = [1; BLOCK];
    cached.dev.disk.write(3, &old).unwrap();
    let mut buf: Block = [0; BLOCK];
    cached.read(3, &mut buf).unwrap();
    assert!(cached.write(3, &[2; BLOCK]).is_err());
    cached.read(3, &mut buf).unwrap();
    assert_eq!(
        buf[..],
        old[..],
        "the refused write was served from the cache"
    );
}

#[test]
fn a_tree_read_again_comes_from_memory() {
    let mut fs = Fs::mkfs(
        Cached::new(MemDisk::new(16384), kaifs::cache::DEFAULT_BLOCKS),
        [1; 16],
        "hits",
    )
    .unwrap();
    fs.set_volume(HOME).unwrap();
    for i in 0..2_000 {
        let f = fs.create(ROOT, &format!("file {i}")).unwrap();
        fs.write_file(f, &[i as u8; 300]).unwrap();
    }
    fs.commit().unwrap();
    let mut fs = Fs::mount(fs.into_dev()).unwrap();
    fs.set_volume(HOME).unwrap();
    let walk = |fs: &mut Fs<Cached<MemDisk>>| {
        let names = fs.list(ROOT, None, usize::MAX).unwrap();
        for l in &names {
            fs.read_file(l.entry.ino).unwrap();
        }
        names.len()
    };
    assert_eq!(walk(&mut fs), 2_000);
    let (hits, misses) = (fs.io.dev.hits, fs.io.dev.misses);
    walk(&mut fs);
    let (again_hits, again_misses) = (fs.io.dev.hits - hits, fs.io.dev.misses - misses);
    eprintln!(
        "kaifs cache: the second walk of 2,000 files: {again_hits} hits, {again_misses} misses"
    );
    assert_eq!(again_misses, 0);
    assert!(again_hits > 1_000);
}
