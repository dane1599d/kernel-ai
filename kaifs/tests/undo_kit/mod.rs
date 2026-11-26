//! What kaifs's undo tests share: a fresh image, a check of it, and paths.
#![allow(dead_code)]

use kaifs::check::{Cache, check};
use kaifs::ns_check::check_namespace;
use kaifs::undo::Outcome;
use kaifs::{Fs, MemDisk};

pub const BLOCKS: u64 = 8192;

pub fn fresh() -> Fs<MemDisk> {
    Fs::mkfs(MemDisk::new(BLOCKS), [6; 16], "undo").unwrap()
}

pub fn remount(fs: Fs<MemDisk>) -> Fs<MemDisk> {
    Fs::mount(fs.into_dev()).unwrap()
}

pub fn checked<D: kaifs::BlockDev>(fs: &mut Fs<D>) {
    let mut cache = Cache::default();
    check(fs, &mut cache).unwrap();
    check_namespace(fs, &mut cache).unwrap();
}

pub fn split(path: &str) -> (&str, &str) {
    let at = path.rfind('/').unwrap();
    (if at == 0 { "/" } else { &path[..at] }, &path[at + 1..])
}

/// Writes `data` at `path`, making the file if needed; returns its inode.
pub fn write<D: kaifs::BlockDev>(fs: &mut Fs<D>, path: &str, data: &[u8]) -> u64 {
    let (dir, name) = split(path);
    let dir = fs.resolve(dir).unwrap();
    let ino = match fs.lookup(dir, name).unwrap() {
        Some(e) => e.ino,
        None => fs.create(dir, name).unwrap(),
    };
    fs.write_file(ino, data).unwrap();
    ino
}

pub fn read(fs: &mut Fs<MemDisk>, path: &str) -> Option<Vec<u8>> {
    let ino = fs.resolve(path).ok()?;
    fs.read_file(ino).ok()
}

pub fn mkdir(fs: &mut Fs<MemDisk>, path: &str) -> u64 {
    let (dir, name) = split(path);
    let dir = fs.resolve(dir).unwrap();
    fs.mkdir(dir, name).unwrap()
}

pub fn read_on<D: kaifs::BlockDev>(fs: &mut Fs<D>, path: &str) -> Option<Vec<u8>> {
    let ino = fs.resolve(path).ok()?;
    fs.read_file(ino).ok()
}

/// One action: `act` on volume `vol`, committed. Returns its generation.
pub fn action(fs: &mut Fs<MemDisk>, id: u64, act: impl FnOnce(&mut Fs<MemDisk>)) -> u64 {
    fs.begin(id).unwrap();
    act(fs);
    fs.commit().unwrap()
}

/// Restores `txn` as an action of its own; returns the outcomes and its commit.
pub fn undo(fs: &mut Fs<MemDisk>, txn: u64) -> (Vec<(u64, Outcome)>, u64) {
    fs.begin(9_000 + txn).unwrap();
    let out = fs.restore(txn).unwrap();
    (out, fs.commit().unwrap())
}
