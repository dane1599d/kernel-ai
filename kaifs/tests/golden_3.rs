//! Golden image 3 (kaifs hardening), `tests/golden/format-1.0-history.kgold`:
//! what images 1 and 2 leave out. An allocator chain condensed into a new base
//! with runs still pending; the trash, with its `info.cbor`; update and package
//! snapshots, pinned and expiring; an UNDO record split over parts; an undo
//! snapshot dropped by retention and a GC pass after it; a 255-byte name; and a
//! file 1 TiB long with one page written. Every later build must mount it,
//! check it, read it back and write on it.
//!
//! And the encoders frozen: every superblock, node, volume-table entry, UNDO
//! record, inode, name chain and attribute chain in all three images, and the
//! trash's `info.cbor`, decodes and encodes back to the bytes it was read from,
//! so a writer whose encoding drifts fails here even while its reader still
//! mounts the images. Rewriting image 3 is for a new format version only:
//! `cargo test -p kaifs --test golden_3 -- --ignored write_golden_image_3`

#[path = "common/golden_io.rs"]
mod golden_io;

use golden_io::{read_image, write_image};
use kaifs::check::{Cache, check};
use kaifs::format::{self, BlockPtr, Superblock};
use kaifs::fs::{HOME, SYSTEM, inline_of, record_of};
use kaifs::items::{
    Inode, K_DATA, K_DIRENT, K_INODE, K_XATTR, decode_attrs, decode_names, encode_attrs,
    encode_names,
};
use kaifs::ns_check::check_namespace;
use kaifs::trash::{INFO, decode_info, encode_info};
use kaifs::tree::{Child, Key, Node};
use kaifs::vol::{
    K_SNAPSHOT, K_UNDO, K_VOLUME, SnapKind, UNDO_LIFE, decode_snapshot, decode_undo, decode_volume,
    encode_snapshot, encode_undo, encode_volume,
};
use kaifs::{BlockDev, Fs, MemDisk, ROOT};
use std::collections::{BTreeMap, BTreeSet};

const IMAGE_1: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/format-1.0.kgold");
const IMAGE_2: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/golden/format-1.0-volumes.kgold"
);
const IMAGE_3: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/golden/format-1.0-history.kgold"
);
const BLOCKS_3: u64 = 1024;
const UUID_3: [u8; 16] = *b"kaifs golden 1.0";
/// 2026-10-26 00:00:00 UTC, as in images 1 and 2.
const EPOCH: u64 = 1_792_972_800_000_000_000;
const DAY: u64 = 24 * 3600 * 1_000_000_000;
/// Where the long file's one page lies: 1 TiB in.
const FAR: u64 = 1 << 40;
/// The files action 303 makes, so its UNDO record takes several parts.
const BULK: u64 = 300;

fn second(s: u64) -> u64 {
    EPOCH + s * 1_000_000_000
}

/// A name of exactly 255 bytes of UTF-8: the longest a name may be.
fn long_name() -> String {
    let name = format!("{}x", "\u{e9}".repeat(127));
    assert_eq!(name.len(), 255);
    name
}

/// Image 3's content, by a generator of its own so it never changes with the
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

fn at(fs: &mut Fs<MemDisk>, path: &str) -> u64 {
    fs.resolve(path).unwrap()
}

fn put(fs: &mut Fs<MemDisk>, dir: u64, name: &str, data: &[u8]) -> u64 {
    let ino = match fs.lookup(dir, name).unwrap() {
        Some(e) => e.ino,
        None => fs.create(dir, name).unwrap(),
    };
    fs.write_file(ino, data).unwrap();
    ino
}

fn commit_at(fs: &mut Fs<MemDisk>, s: u64) -> u64 {
    fs.clock = second(s);
    fs.commit().unwrap()
}

/// Image 3's content; its generations are 1 (mkfs) to 81.
fn script_3(fs: &mut Fs<MemDisk>) {
    fs.set_volume(HOME).unwrap();
    let user = fs.mkdir(ROOT, "user").unwrap();
    put(fs, user, "keep.txt", b"kept\n");
    put(fs, user, "old.txt", b"old, then trashed\n");
    commit_at(fs, 2);
    // Action 301 edits a file; retention drops its undo snapshot below.
    fs.begin(301).unwrap();
    put(fs, user, "keep.txt", b"kept, edited\n");
    commit_at(fs, 3);
    // Action 302 trashes a file.
    fs.begin(302).unwrap();
    fs.clock = second(4);
    fs.trash(user, "old.txt").unwrap();
    commit_at(fs, 4);
    // Action 303 makes a folder of 300 files.
    fs.begin(303).unwrap();
    let bulk = fs.mkdir(user, "bulk").unwrap();
    for i in 0..BULK {
        put(fs, bulk, &format!("f{i:03}"), &bytes(i, 8));
    }
    commit_at(fs, 5);
    put(fs, user, &long_name(), b"a long name\n");
    let far = fs.create(user, "far.img").unwrap();
    fs.write_at(far, FAR, &bytes(999, 4096)).unwrap();
    commit_at(fs, 6);
    // system: update and package snapshots.
    fs.set_volume(SYSTEM).unwrap();
    let state = fs.mkdir(ROOT, "state").unwrap();
    put(fs, state, "counter", b"start");
    commit_at(fs, 7);
    fs.snapshot_as(SnapKind::Update, 0, 7).unwrap();
    fs.snapshot_as(SnapKind::Package, second(8) + 90 * DAY, 12)
        .unwrap();
    commit_at(fs, 8);
    // 70 commits: at its 64th delta the allocator's chain becomes a new base,
    // which holds the runs the commit before it freed, still pending.
    for n in 0..70 {
        put(fs, state, "counter", format!("{n}").as_bytes());
        commit_at(fs, 9 + n);
    }
    // 30 days after action 301, its undo snapshot expires; a GC pass then
    // frees what only it held.
    let dropped = fs.expire(second(3) + UNDO_LIFE).unwrap();
    assert_eq!(dropped.len(), 1);
    commit_at(fs, 79);
    assert!(fs.gc(64).unwrap() > 0);
    commit_at(fs, 80);
    fs.set_volume(HOME).unwrap();
    put(fs, user, "keep.txt", b"kept, edited twice\n");
    commit_at(fs, 81);
}

#[test]
#[ignore = "rewrites golden image 3: only a new format version should"]
fn write_golden_image_3() {
    let mut fs = Fs::mkfs(MemDisk::new(BLOCKS_3), UUID_3, "golden-3").unwrap();
    script_3(&mut fs);
    write_image(IMAGE_3, &fs.into_dev(), BLOCKS_3);
}

fn checks(fs: &mut Fs<MemDisk>) {
    let mut cache = Cache::default();
    check(fs, &mut cache).unwrap();
    check_namespace(fs, &mut cache).unwrap();
}

#[test]
fn golden_image_3_mounts_checks_and_reads_back() {
    let mut fs = Fs::mount(read_image(IMAGE_3)).unwrap();
    assert_eq!(fs.committed, 81);
    checks(&mut fs);
    // The chain was condensed: a base and the deltas since, not 80 deltas.
    assert!(fs.io.alloc.chain().len() < 30, "a condensed chain");
    fs.set_volume(HOME).unwrap();
    let keep = at(&mut fs, "/user/keep.txt");
    assert_eq!(fs.read_file(keep).unwrap(), b"kept, edited twice\n");
    assert!(fs.resolve("/user/old.txt").is_err());
    // Retention dropped action 301's undo; 302's and 303's are left.
    let records = fs.undo_records().unwrap();
    let actions: Vec<u64> = records.iter().map(|u| u.action).collect();
    assert_eq!(actions, [302, 303]);
    assert!(encode_undo(&records[1]).unwrap().len() >= 2, "parts");
    assert!(records[1].objects.len() as u64 > BULK);
    let kinds: Vec<_> = fs.snapshots().values().map(|s| (s.kind, s.pin)).collect();
    assert_eq!(kinds, [(SnapKind::Undo, 302), (SnapKind::Undo, 303)]);
    // The trash.
    let trashed = fs.trash_list().unwrap();
    assert_eq!(trashed.len(), 1);
    assert_eq!(trashed[0].name, "old.txt");
    assert_eq!(trashed[0].info.path, "/user/old.txt");
    assert_eq!(
        (trashed[0].info.deleted, trashed[0].info.action),
        (second(4), 302)
    );
    assert_eq!(
        fs.read_file(trashed[0].ino).unwrap(),
        b"old, then trashed\n"
    );
    let info = fs.lookup(trashed[0].folder, INFO).unwrap().unwrap().ino;
    let cbor = fs.read_file(info).unwrap();
    assert_eq!(encode_info(&decode_info(&cbor).unwrap()), cbor, "frozen");
    // The bulk folder, the long name and the long file.
    let bulk = at(&mut fs, "/user/bulk");
    assert_eq!(fs.stat(bulk).unwrap().size, BULK);
    let f = at(&mut fs, "/user/bulk/f123");
    assert_eq!(fs.read_file(f).unwrap(), bytes(123, 8));
    let user = at(&mut fs, "/user");
    let long = fs.lookup(user, &long_name()).unwrap().unwrap().ino;
    assert_eq!(fs.read_file(long).unwrap(), b"a long name\n");
    let far = at(&mut fs, "/user/far.img");
    assert_eq!(fs.stat(far).unwrap().size, FAR + 4096);
    assert_eq!(fs.read_at(far, FAR, 4096).unwrap(), bytes(999, 4096));
    assert_eq!(fs.read_at(far, FAR - 8, 8).unwrap(), [0; 8]);
    // system: the update and package snapshots, and the counter.
    fs.set_volume(SYSTEM).unwrap();
    let snaps: Vec<_> = fs
        .snapshots()
        .values()
        .map(|s| (s.kind, s.pin, s.expiry))
        .collect();
    assert_eq!(
        snaps,
        [
            (SnapKind::Update, 7, 0),
            (SnapKind::Package, 12, second(8) + 90 * DAY)
        ]
    );
    let counter = at(&mut fs, "/state/counter");
    assert_eq!(fs.read_file(counter).unwrap(), b"69");
    // A later build writes on it: it restores action 302, which brings the
    // file back from the trash; runs a GC pass; and empties the trash.
    fs.set_volume(HOME).unwrap();
    fs.begin(400).unwrap();
    fs.restore(records[0].txn).unwrap();
    fs.commit().unwrap();
    let old = at(&mut fs, "/user/old.txt");
    assert_eq!(fs.read_file(old).unwrap(), b"old, then trashed\n");
    fs.gc(64).unwrap();
    fs.commit().unwrap();
    fs.empty_trash().unwrap();
    fs.commit().unwrap();
    assert!(fs.trash_list().unwrap().is_empty());
    let mut fs = Fs::mount(fs.into_dev()).unwrap();
    checks(&mut fs);
}

/// Each node under `ptr` not seen yet, which must encode back to its block,
/// and each item of its leaves.
fn walk(
    disk: &MemDisk,
    ptr: &BlockPtr,
    seen: &mut BTreeSet<u64>,
    count: &mut u64,
    visit: &mut dyn FnMut(&Key, &[u8]),
) {
    if ptr.is_null() || !seen.insert(ptr.addr) {
        return;
    }
    let block = disk.get(ptr.addr).expect("a node the image holds");
    let node = Node::decode(block).unwrap();
    assert_eq!(node.encode(), **block, "node {}", ptr.addr);
    *count += 1;
    match node {
        Node::Leaf(items) => items.iter().for_each(|(key, raw)| visit(key, raw)),
        Node::Branch(_, children) => {
            for (_, child) in children {
                let Child::Clean(child) = child else {
                    unreachable!("a node read from disk is clean");
                };
                walk(disk, &child, seen, count, visit);
            }
        }
    }
}

/// Checks that `again`, a structure of the image at `path` decoded and encoded,
/// is the `raw` bytes it was read from. One an older writer left shorter
/// encodes to its bytes and the fields added since: inodes and volume and
/// snapshot entries grew at the end in task 6, after image 1, the only image
/// that may hold one.
fn same(path: &str, raw: &[u8], again: &[u8], grown: &mut u64) {
    if again != raw {
        assert!(
            path == IMAGE_1 && again.starts_with(raw),
            "{path}: {raw:?} encodes back as {again:?}"
        );
        *grown += 1;
    }
}

/// How many of each structure in the image at `path` encoded back to its
/// bytes.
fn reencoded(path: &str) -> BTreeMap<&'static str, u64> {
    let disk = read_image(path);
    let mut counts: BTreeMap<&'static str, u64> = BTreeMap::new();
    for addr in format::all_slots(disk.info().blocks) {
        let Some(block) = disk.get(addr) else {
            continue;
        };
        if let Ok(sb) = Superblock::decode(block) {
            assert_eq!(sb.encode(), **block, "{path}: superblock slot {addr}");
            *counts.entry("superblocks").or_default() += 1;
        }
    }
    let mut fs = Fs::mount(disk.clone()).unwrap();
    let mut seen = BTreeSet::new();
    let mut nodes = 0;
    let mut undo: BTreeMap<(u64, u64), Vec<Vec<u8>>> = BTreeMap::new();
    let (mut entries, mut grown) = (0, 0);
    let volumes = fs.volumes_root().unwrap();
    walk(&disk, &volumes, &mut seen, &mut nodes, &mut |key, raw| {
        entries += 1;
        match key.kind {
            K_VOLUME => {
                let entry = decode_volume(key.oid, raw).unwrap();
                same(path, raw, &encode_volume(&entry), &mut grown);
            }
            K_SNAPSHOT => {
                let snap = decode_snapshot(raw).unwrap();
                same(path, raw, &encode_snapshot(&snap), &mut grown);
            }
            K_UNDO => undo
                .entry((key.oid, key.off >> 16))
                .or_default()
                .push(raw.to_vec()),
            kind => panic!("{path}: a volume table item of kind {kind}"),
        }
    });
    counts.insert("volume table entries", entries);
    for ((vol, txn), parts) in &undo {
        let record = decode_undo(*txn, parts).unwrap();
        let again = encode_undo(&record).unwrap();
        assert_eq!(&again, parts, "{path}: volume {vol}'s UNDO record {txn}");
    }
    counts.insert("UNDO records", undo.len() as u64);
    let mut items: BTreeMap<&'static str, u64> = BTreeMap::new();
    for (id, _) in fs.volumes() {
        fs.set_volume(id).unwrap();
        let mut roots = vec![fs.live_root().unwrap()];
        roots.extend(fs.snapshots().values().map(|s| s.root));
        for root in roots {
            walk(&disk, &root, &mut seen, &mut nodes, &mut |key, raw| {
                let inline = inline_of(raw);
                let what = match key.kind {
                    K_INODE => {
                        let bytes = inline.unwrap();
                        let inode = Inode::decode(bytes).unwrap();
                        same(path, bytes, &inode.encode(), &mut grown);
                        "inodes"
                    }
                    K_DIRENT => {
                        let bytes = inline.unwrap();
                        assert_eq!(encode_names(&decode_names(bytes).unwrap()), bytes);
                        "name chains"
                    }
                    K_XATTR => {
                        let bytes = inline.unwrap();
                        assert_eq!(encode_attrs(&decode_attrs(bytes).unwrap()), bytes);
                        "attribute chains"
                    }
                    K_DATA if record_of(raw).is_some() => "data records",
                    K_DATA => "inline data",
                    kind => panic!("{path}: an item of kind {kind}"),
                };
                *items.entry(what).or_default() += 1;
            });
        }
    }
    counts.extend(items);
    counts.insert("nodes", nodes);
    counts.insert("grown since", grown);
    counts
}

#[test]
fn every_structure_in_the_golden_images_encodes_back_to_its_bytes() {
    for path in [IMAGE_1, IMAGE_2, IMAGE_3] {
        let counts = reencoded(path);
        eprintln!("{path}: {counts:?}");
        for what in ["superblocks", "nodes", "inodes", "name chains"] {
            assert!(
                counts.get(what).is_some_and(|&n| n > 0),
                "{path}: no {what}"
            );
        }
    }
}
