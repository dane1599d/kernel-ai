//! Golden image 1 (ADR 0004 §2; P2 task 5): a format 1.0 filesystem, written by
//! the build that froze format 1 and committed as bytes in
//! `tests/golden/format-1.0.kgold`. Every later build must mount it, pass
//! `check()` and the namespace rules, and read back exactly what [`script`]
//! wrote, on the live tree and in its snapshot.
//!
//! The image holds something of every format 1 structure: both superblock rings
//! after the slots wrap, the volume table with a live entry and a snapshot, the
//! allocator's chain, a tree of more than one level, inline data, one-record and
//! two-record files, a file with holes, an empty file, attributes, a renamed
//! file and folder, removed files, and a UTF-8 name.
//!
//! Golden image 2, `tests/golden/format-1.0-volumes.kgold`, adds task 6's
//! structures: three volumes, actions with undo snapshots and UNDO records, a
//! restore, and inodes that say when they changed.
//!
//! The file is a sparse dump: magic `KAIGOLD1`, the block count (u64), then each
//! written block as its address (u64), a count of segments (u16) and each segment
//! as an offset (u16), a length (u16) and its bytes; the zeros between segments
//! are left out. Rewriting it is for a new format version only:
//! `cargo test -p kaifs --test golden -- --ignored write_golden_image_1`

use kaifs::check::{Cache, check};
use kaifs::format::{self, FORMAT_MAJOR, FORMAT_MINOR};
use kaifs::ns_check::{Digest, check_namespace};
use kaifs::{BLOCK, BlockDev, FileType, Fs, MemDisk, ROOT};
use std::collections::BTreeMap;

#[path = "common/golden_io.rs"]
mod golden_io;

use golden_io::{read_image, write_image};

const IMAGE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/format-1.0.kgold");
const BLOCKS: u64 = 1024;
const UUID: [u8; 16] = *b"kaifs golden 1.0";
/// 2026-10-26 00:00:00 UTC, the clock of the first commit.
const EPOCH: u64 = 1_792_972_800_000_000_000;

/// The golden content, by a generator of its own so it never changes with the
/// crate's.
fn bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (x >> 56) as u8
        })
        .collect()
}

/// `len` bytes whose 4 KiB pages each start with 48 bytes of content and are
/// zeros after, so the image stays small and every page still differs.
fn paged(seed: u64, len: usize) -> Vec<u8> {
    let mut out = vec![0; len];
    for (i, page) in out.chunks_mut(BLOCK).enumerate() {
        let n = page.len().min(48);
        page[..n].copy_from_slice(&bytes(seed + i as u64, n));
    }
    out
}

/// What the script does, to a filesystem or to a model of one.
trait Target {
    fn mkdir(&mut self, path: &str);
    fn write(&mut self, path: &str, data: &[u8]);
    fn write_at(&mut self, path: &str, off: u64, data: &[u8]);
    fn xattr(&mut self, path: &str, name: &str, value: &[u8]);
    fn rename(&mut self, from: &str, to: &str);
    fn remove(&mut self, path: &str);
    fn snapshot(&mut self);
    fn commit(&mut self, at: u64);
}

/// Golden image 1's content. Changing it breaks the test until the image is
/// written again, which is the point.
fn script(t: &mut dyn Target) {
    for dir in [
        "/home",
        "/home/user",
        "/home/.trash",
        "/system",
        "/system/settings",
        "/system/state",
        "/system/pkg",
    ] {
        t.mkdir(dir);
    }
    t.write("/home/user/hello.txt", b"Hello from kaifs format 1.\n");
    t.write("/home/user/notes.md", &paged(1, 6000));
    t.write("/home/user/photo.bin", &paged(2, 200 << 10));
    t.xattr(
        "/home/user/photo.bin",
        "user.mime",
        b"application/octet-stream",
    );
    t.write("/home/user/empty", b"");
    t.write(
        "/home/user/\u{65e5}\u{8a18}.txt",
        "kaifs \u{306e}\u{65e5}\u{8a18}\n".as_bytes(),
    );
    t.write("/home/user/sparse.img", b"");
    t.write_at("/home/user/sparse.img", 1 << 20, &bytes(3, 4096));
    t.write_at("/home/user/sparse.img", 5, b"start");
    t.write(
        "/system/settings/display.cbor",
        &[0x82, 0x01, 0xA1, 0x65, b's', b'c', b'a', b'l', b'e', 0x02],
    );
    t.commit(1);
    t.mkdir("/home/user/many");
    for i in 0..200u64 {
        t.write(
            &format!("/home/user/many/f{i:03}"),
            &bytes(100 + i, 8 + (i % 16) as usize),
        );
    }
    t.commit(2);
    t.snapshot();
    t.commit(3);
    t.mkdir("/home/user/docs");
    t.rename("/home/user/notes.md", "/home/user/docs/notes.md");
    t.rename("/home/user/many", "/home/user/docs/many");
    t.write("/home/user/hello.txt", b"Hello again, after a snapshot.\n");
    t.commit(4);
    for i in 150..200 {
        t.remove(&format!("/home/user/docs/many/f{i:03}"));
    }
    t.xattr("/home/user/docs", "user.note", b"moved here");
    t.commit(5);
    for at in 6..12 {
        t.write("/system/state/counter", format!("{at}").as_bytes());
        t.commit(at);
    }
}

struct FsTarget(Fs<MemDisk>);

fn split(path: &str) -> (&str, &str) {
    let at = path.rfind('/').unwrap();
    (if at == 0 { "/" } else { &path[..at] }, &path[at + 1..])
}

impl FsTarget {
    fn ino(&mut self, path: &str) -> u64 {
        self.0.resolve(path).unwrap()
    }
}

impl Target for FsTarget {
    fn mkdir(&mut self, path: &str) {
        let (dir, name) = split(path);
        let dir = self.ino(dir);
        self.0.mkdir(dir, name).unwrap();
    }

    fn write(&mut self, path: &str, data: &[u8]) {
        let (dir, name) = split(path);
        let dir = self.ino(dir);
        let ino = match self.0.lookup(dir, name).unwrap() {
            Some(e) => e.ino,
            None => self.0.create(dir, name).unwrap(),
        };
        self.0.write_file(ino, data).unwrap();
    }

    fn write_at(&mut self, path: &str, off: u64, data: &[u8]) {
        let ino = self.ino(path);
        self.0.write_at(ino, off, data).unwrap();
    }

    fn xattr(&mut self, path: &str, name: &str, value: &[u8]) {
        let ino = self.ino(path);
        self.0.set_xattr(ino, name, value).unwrap();
    }

    fn rename(&mut self, from: &str, to: &str) {
        let ((fd, fname), (td, tname)) = (split(from), split(to));
        let (fd, td) = (self.ino(fd), self.ino(td));
        self.0.rename(fd, fname, td, tname).unwrap();
    }

    fn remove(&mut self, path: &str) {
        let (dir, name) = split(path);
        let dir = self.ino(dir);
        self.0.remove(dir, name).unwrap();
    }

    fn snapshot(&mut self) {
        self.0.snapshot().unwrap();
    }

    fn commit(&mut self, at: u64) {
        self.0.clock = EPOCH + at * 1_000_000_000;
        self.0.commit().unwrap();
    }
}

#[derive(Clone, Debug)]
enum Node {
    Dir,
    File(Vec<u8>),
}

#[derive(Clone, Default)]
struct Ns {
    nodes: BTreeMap<String, Node>,
    attrs: BTreeMap<String, BTreeMap<String, Vec<u8>>>,
}

impl Ns {
    fn digest(&self) -> Digest {
        let mut d = Digest::default();
        d.dir("/");
        for (path, node) in &self.nodes {
            match node {
                Node::Dir => d.dir(path),
                Node::File(data) => {
                    d.file(path, data.len() as u64);
                    d.bytes(path, data.len() as u64, 0, data);
                }
            }
        }
        for (path, attrs) in &self.attrs {
            for (name, value) in attrs {
                d.attr(path, name, value);
            }
        }
        d
    }
}

#[derive(Default)]
struct Model {
    live: Ns,
    snaps: Vec<Ns>,
    commits: u64,
}

impl Target for Model {
    fn mkdir(&mut self, path: &str) {
        self.live.nodes.insert(path.into(), Node::Dir);
    }

    fn write(&mut self, path: &str, data: &[u8]) {
        self.live
            .nodes
            .insert(path.into(), Node::File(data.to_vec()));
    }

    fn write_at(&mut self, path: &str, off: u64, data: &[u8]) {
        let Some(Node::File(bytes)) = self.live.nodes.get_mut(path) else {
            panic!("{path} is not a file");
        };
        let end = off as usize + data.len();
        if bytes.len() < end {
            bytes.resize(end, 0);
        }
        bytes[off as usize..end].copy_from_slice(data);
    }

    fn xattr(&mut self, path: &str, name: &str, value: &[u8]) {
        let attrs = self.live.attrs.entry(path.into()).or_default();
        attrs.insert(name.into(), value.to_vec());
    }

    fn rename(&mut self, from: &str, to: &str) {
        let moved: Vec<String> = self
            .live
            .nodes
            .keys()
            .filter(|p| *p == from || p.starts_with(&format!("{from}/")))
            .cloned()
            .collect();
        for old in moved {
            let new = format!("{to}{}", &old[from.len()..]);
            let node = self.live.nodes.remove(&old).unwrap();
            self.live.nodes.insert(new.clone(), node);
            if let Some(attrs) = self.live.attrs.remove(&old) {
                self.live.attrs.insert(new, attrs);
            }
        }
    }

    fn remove(&mut self, path: &str) {
        self.live.nodes.remove(path);
        self.live.attrs.remove(path);
    }

    fn snapshot(&mut self) {
        self.snaps.push(self.live.clone());
    }

    fn commit(&mut self, _: u64) {
        self.commits += 1;
    }
}

#[test]
#[ignore = "rewrites golden image 1: only a new format version should"]
fn write_golden_image_1() {
    let mut fs = Fs::mkfs(MemDisk::new(BLOCKS), UUID, "golden-1").unwrap();
    let mut target = FsTarget(fs);
    script(&mut target);
    fs = target.0;
    write_image(IMAGE, &fs.into_dev(), BLOCKS);
}

#[test]
fn golden_image_1_mounts_checks_and_reads_back() {
    let mut model = Model::default();
    script(&mut model);
    let mut disk = read_image(IMAGE);
    let found = format::read_rings(&mut disk).unwrap();
    assert_eq!((found[0].format_major, found[0].format_minor), (1, 0));
    assert!((FORMAT_MAJOR, FORMAT_MINOR) >= (1, 0));
    let mut fs = Fs::mount(disk).unwrap();
    assert_eq!(
        fs.committed,
        1 + model.commits,
        "mkfs, then the script's commits"
    );
    assert_eq!(fs.uuid(), UUID);
    let mut cache = Cache::default();
    let report = check(&mut fs, &mut cache).unwrap();
    let ns = check_namespace(&mut fs, &mut cache).unwrap();
    let ns = &ns.volumes[&1];
    assert_eq!(ns.live.digest, model.live.digest(), "the live namespace");
    assert_eq!(ns.snaps.len(), 1);
    let snap = ns.snaps.values().next().unwrap();
    assert_eq!(snap.digest, model.snaps[0].digest(), "the snapshot");
    assert!(
        report.volumes[&1].live.root.level >= 1,
        "the tree has more than one level"
    );
    // A few reads by hand, beside the digest.
    let hello = fs.resolve("/home/user/hello.txt").unwrap();
    assert_eq!(
        fs.read_file(hello).unwrap(),
        b"Hello again, after a snapshot.\n"
    );
    let sparse = fs.resolve("/home/user/sparse.img").unwrap();
    assert_eq!(fs.stat(sparse).unwrap().size, (1 << 20) + 4096);
    assert_eq!(fs.read_at(sparse, 0, 10).unwrap(), b"\0\0\0\0\0start");
    let photo = fs.resolve("/home/user/photo.bin").unwrap();
    assert_eq!(fs.read_file(photo).unwrap(), paged(2, 200 << 10));
    assert_eq!(
        fs.get_xattr(photo, "user.mime").unwrap().unwrap(),
        b"application/octet-stream"
    );
    let many = fs.resolve("/home/user/docs/many").unwrap();
    assert_eq!(fs.stat(many).unwrap().size, 150);
    assert_eq!(fs.stat(ROOT).unwrap().kind, FileType::Dir);
    assert!(fs.resolve("/home/user/notes.md").is_err());
    // The image is a disk a later build writes on.
    let f = fs.create(ROOT, "later").unwrap();
    fs.write_file(f, b"written by a later build").unwrap();
    fs.commit().unwrap();
    let mut cache = Cache::default();
    check(&mut fs, &mut cache).unwrap();
    check_namespace(&mut fs, &mut cache).unwrap();
    let mut written = fs.into_dev();
    assert!(written.flush().is_ok());
}

/// Golden image 2 (task 6): the three volumes, actions with their undo snapshots
/// and UNDO records across volumes, a restore, a named snapshot, a dropped undo
/// snapshot, and inodes that say when they changed.
const IMAGE_2: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/golden/format-1.0-volumes.kgold"
);
const BLOCKS_2: u64 = 512;
const UUID_2: [u8; 16] = *b"kaifs golden 1.0";

fn at(fs: &mut Fs<MemDisk>, path: &str) -> u64 {
    fs.resolve(path).unwrap()
}

fn put(fs: &mut Fs<MemDisk>, path: &str, data: &[u8]) -> u64 {
    let (dir, name) = split(path);
    let dir = at(fs, dir);
    let ino = match fs.lookup(dir, name).unwrap() {
        Some(e) => e.ino,
        None => fs.create(dir, name).unwrap(),
    };
    fs.write_file(ino, data).unwrap();
    ino
}

fn commit_at(fs: &mut Fs<MemDisk>, second: u64) -> u64 {
    fs.clock = EPOCH + second * 1_000_000_000;
    fs.commit().unwrap()
}

/// Golden image 2's content; its generations are 1 (mkfs) to 8.
fn script_2(fs: &mut Fs<MemDisk>) {
    use kaifs::fs::{HOME, MODELS, SYSTEM};
    fs.set_volume(SYSTEM).unwrap();
    fs.mkdir(ROOT, "settings").unwrap();
    put(fs, "/settings/display.cbor", &[0x82, 0x01, 0xA0]);
    fs.set_volume(HOME).unwrap();
    fs.mkdir(ROOT, "user").unwrap();
    put(fs, "/user/notes.txt", b"first notes\n");
    put(fs, "/user/photo.bin", &paged(5, 9000));
    fs.set_volume(MODELS).unwrap();
    fs.mkdir(ROOT, "cache").unwrap();
    commit_at(fs, 2);
    // Action 101 edits a note, adds a list, and logs it in system: one commit.
    fs.begin(101).unwrap();
    fs.set_volume(HOME).unwrap();
    put(fs, "/user/notes.txt", b"second notes\n");
    put(fs, "/user/todo.txt", b"buy milk\n");
    fs.set_volume(SYSTEM).unwrap();
    put(fs, "/log", b"101 edited notes\n");
    commit_at(fs, 3);
    // Action 102 removes the list and moves the photo into a new folder.
    fs.begin(102).unwrap();
    fs.set_volume(HOME).unwrap();
    let user = at(fs, "/user");
    fs.remove(user, "todo.txt").unwrap();
    let pictures = fs.mkdir(user, "pictures").unwrap();
    fs.rename(user, "photo.bin", pictures, "photo.bin").unwrap();
    commit_at(fs, 4);
    fs.snapshot().unwrap();
    commit_at(fs, 5);
    // Action 103 restores action 101 in home: the first notes come back.
    let txn_101 = fs.undo_records().unwrap()[0].txn;
    fs.begin(103).unwrap();
    fs.restore(txn_101).unwrap();
    commit_at(fs, 6);
    // Action 104 logs again; then system's oldest undo snapshot is dropped.
    fs.begin(104).unwrap();
    fs.set_volume(SYSTEM).unwrap();
    put(fs, "/log", b"101 edited notes\n104 logged\n");
    commit_at(fs, 7);
    let oldest = *fs.snapshots().keys().next().unwrap();
    fs.drop_snapshot(oldest).unwrap();
    fs.set_volume(HOME).unwrap();
    let notes = at(fs, "/user/notes.txt");
    fs.set_xattr(notes, "user.mood", b"calm").unwrap();
    commit_at(fs, 8);
}

#[test]
#[ignore = "rewrites golden image 2: only a new format version should"]
fn write_golden_image_2() {
    let mut fs = Fs::mkfs(MemDisk::new(BLOCKS_2), UUID_2, "golden-2").unwrap();
    script_2(&mut fs);
    write_image(IMAGE_2, &fs.into_dev(), BLOCKS_2);
}

#[test]
fn golden_image_2_mounts_checks_and_reads_back() {
    use kaifs::fs::{HOME, MODELS, SYSTEM};
    use kaifs::vol::{SnapKind, UNDO_LIFE};
    let mut fs = Fs::mount(read_image(IMAGE_2)).unwrap();
    assert_eq!(fs.committed, 8, "mkfs, then the script's seven commits");
    let names: Vec<String> = fs.volumes().into_iter().map(|(_, n)| n).collect();
    assert_eq!(names, ["system", "home", "models"]);
    let mut cache = Cache::default();
    check(&mut fs, &mut cache).unwrap();
    check_namespace(&mut fs, &mut cache).unwrap();
    let second = |s: u64| EPOCH + s * 1_000_000_000;
    // system: the settings, the log, and one undo snapshot left, action 104's.
    fs.set_volume(SYSTEM).unwrap();
    let display = at(&mut fs, "/settings/display.cbor");
    assert_eq!(fs.read_file(display).unwrap(), [0x82, 0x01, 0xA0]);
    let log = at(&mut fs, "/log");
    assert_eq!(
        fs.read_file(log).unwrap(),
        b"101 edited notes\n104 logged\n"
    );
    let snaps: Vec<_> = fs
        .snapshots()
        .values()
        .map(|s| (s.kind, s.pin, s.expiry))
        .collect();
    assert_eq!(snaps, [(SnapKind::Undo, 104, second(7) + UNDO_LIFE)]);
    assert_eq!(
        fs.undo_records()
            .unwrap()
            .iter()
            .map(|u| u.action)
            .collect::<Vec<_>>(),
        [104]
    );
    // home: three actions' undo snapshots and a named one between them.
    fs.set_volume(HOME).unwrap();
    let kinds: Vec<_> = fs.snapshots().values().map(|s| (s.kind, s.pin)).collect();
    assert_eq!(
        kinds,
        [
            (SnapKind::Undo, 101),
            (SnapKind::Undo, 102),
            (SnapKind::Named, 0),
            (SnapKind::Undo, 103)
        ]
    );
    let records = fs.undo_records().unwrap();
    assert_eq!(
        records
            .iter()
            .map(|u| (u.action, u.time))
            .collect::<Vec<_>>(),
        [(101, second(3)), (102, second(4)), (103, second(6))]
    );
    let notes = at(&mut fs, "/user/notes.txt");
    assert_eq!(
        fs.read_file(notes).unwrap(),
        b"first notes\n",
        "action 103 restored 101"
    );
    assert_eq!(fs.get_xattr(notes, "user.mood").unwrap().unwrap(), b"calm");
    let photo = at(&mut fs, "/user/pictures/photo.bin");
    assert_eq!(fs.read_file(photo).unwrap(), paged(5, 9000));
    assert!(fs.resolve("/user/todo.txt").is_err());
    assert!(records[0].objects.contains(&notes));
    assert!(fs.stat(notes).unwrap().changed > records[2].before);
    let versions: Vec<Vec<u8>> = fs
        .history(notes)
        .unwrap()
        .iter()
        .map(|v| fs.read_version(v.snapshot, notes, 0, 64).unwrap())
        .collect();
    assert!(versions.contains(&b"second notes\n".to_vec()));
    // models: the cache folder, untouched by any action.
    fs.set_volume(MODELS).unwrap();
    assert!(fs.snapshots().is_empty());
    let cache_dir = at(&mut fs, "/cache");
    assert_eq!(fs.stat(cache_dir).unwrap().kind, FileType::Dir);
    // A later build restores an action the image recorded: 102's removal and
    // move are undone.
    fs.set_volume(HOME).unwrap();
    fs.begin(200).unwrap();
    fs.restore(records[1].txn).unwrap();
    fs.commit().unwrap();
    let todo = at(&mut fs, "/user/todo.txt");
    assert_eq!(fs.read_file(todo).unwrap(), b"buy milk\n");
    let photo = at(&mut fs, "/user/photo.bin");
    assert_eq!(fs.read_file(photo).unwrap(), paged(5, 9000));
    assert!(fs.resolve("/user/pictures").is_err());
    let mut cache = Cache::default();
    check(&mut fs, &mut cache).unwrap();
    check_namespace(&mut fs, &mut cache).unwrap();
}
