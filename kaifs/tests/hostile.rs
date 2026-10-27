//! Hostile input for format 1's readers (ADR 0004; disks become untrusted input
//! in P7): mutated superblocks and pointers, every field at every extreme value,
//! and rings full of garbage. Each must decode or be refused, never panic, and
//! whatever decodes must encode back to itself.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use hostile::{XorShift, fuzz, sweep_fields};
use kaifs::format::{self, BlockPtr, FLAG_SEALED, KIND_NODE, KIND_RECORD, PTR_BYTES, Superblock};
use kaifs::{BLOCK, Block, BlockDev, CsumKind, MemDisk};

fn block_of(bytes: &[u8]) -> Block {
    let mut block = [0; BLOCK];
    let n = bytes.len().min(BLOCK);
    block[..n].copy_from_slice(&bytes[..n]);
    block
}

/// Gives a mutated superblock a good checksum again, so the field checks behind
/// it run.
fn resign(block: &mut Block) {
    let sum = CsumKind::Seahash64.sum(&block[..BLOCK - 32]);
    block[BLOCK - 32..].copy_from_slice(&sum);
}

fn sample_ptr() -> BlockPtr {
    BlockPtr {
        addr: 1234,
        birth: 56,
        blocks: 32,
        kind: KIND_RECORD,
        level: 0,
        flags: FLAG_SEALED,
        csum: [0xA5; 32],
    }
}

fn sample() -> Block {
    let mut sb = Superblock::new(*b"hostile kaifs fs", 1 << 20, 99);
    sb.volumes = BlockPtr {
        kind: KIND_NODE,
        blocks: 1,
        level: 3,
        flags: 0,
        ..sample_ptr()
    };
    sb.alloc = BlockPtr {
        kind: format::KIND_ALLOC,
        blocks: 1,
        flags: 0,
        ..sample_ptr()
    };
    sb.commit_time = 1_798_000_000_000_000_000;
    sb.label[..4].copy_from_slice(b"home");
    sb.encode()
}

fn decode_is_total(block: &Block) {
    if let Ok(sb) = Superblock::decode(block) {
        assert_eq!(Superblock::decode(&sb.encode()), Ok(sb));
        let _ = sb.rules();
    }
}

#[test]
fn mutated_superblocks_never_panic_and_what_decodes_round_trips() {
    let sample = sample();
    fuzz(&sample, 1, 20_000, |bytes| {
        let mut block = block_of(bytes);
        decode_is_total(&block);
        resign(&mut block);
        decode_is_total(&block);
    });
}

#[test]
fn every_superblock_field_at_every_extreme_value() {
    let sample = sample();
    sweep_fields(&sample, 0..408, |_, bytes| {
        let mut block = block_of(bytes);
        resign(&mut block);
        decode_is_total(&block);
    });
}

fn pointer_is_total(bytes: &[u8]) {
    let mut raw = [0; PTR_BYTES];
    let n = bytes.len().min(PTR_BYTES);
    raw[..n].copy_from_slice(&bytes[..n]);
    if let Ok(ptr) = BlockPtr::decode(&raw) {
        assert_eq!(BlockPtr::decode(&ptr.encode()), Ok(ptr));
    }
    let _ = BlockPtr::decode_at(bytes, 0);
    let _ = BlockPtr::decode_at(bytes, bytes.len().saturating_sub(8));
}

#[test]
fn mutated_pointers_never_panic_and_what_decodes_round_trips() {
    let sample = sample_ptr().encode();
    fuzz(&sample, 2, 50_000, pointer_is_total);
    sweep_fields(&sample, 0..PTR_BYTES, |_, bytes| pointer_is_total(bytes));
}

#[test]
fn rings_of_garbage_never_panic() {
    let mut rng = XorShift::new(3);
    let blocks = 4096;
    for _ in 0..300 {
        let mut disk = MemDisk::new(blocks);
        for addr in format::all_slots(blocks) {
            let mut block = match rng.below(4) {
                0 => [0; BLOCK],
                1 => block_of(&hostile::mutate(&mut rng, &sample())),
                _ => {
                    let mut sb =
                        Superblock::new([rng.below(3) as u8; 16], blocks, rng.below(40) as u64);
                    sb.incompat = (rng.below(8) == 0) as u64;
                    sb.encode()
                }
            };
            if rng.below(2) == 0 {
                resign(&mut block);
            }
            disk.write(addr, &block).unwrap();
        }
        if let Ok(found) = format::read_rings(&mut disk) {
            assert!(!found.is_empty());
            assert!(found.windows(2).all(|w| w[0].generation > w[1].generation));
            assert!(found.iter().all(|sb| sb.uuid == found[0].uuid));
        }
    }
}

/// A chain head of `which` kind, then the same head with its bytes mutated and
/// its pointer's checksum made good, so the replay's own checks run: each must
/// load or be refused, never panic, and what loads must stay inside the disk.
#[test]
fn mutated_allocator_blocks_never_panic() {
    use kaifs::allocator::Allocator;
    let blocks = 1 << 14;
    let area = format::data_area(blocks);
    let kind = CsumKind::Seahash64;
    for txns in [1u64, 5] {
        let mut disk = MemDisk::new(blocks);
        let mut alloc = Allocator::new(area);
        let mut head = BlockPtr::NULL;
        for txn in 1..=txns {
            let run = alloc.alloc((txn % 4) as u8).unwrap();
            alloc.free_later(txn, run);
            head = alloc.persist(&mut disk, kind, txn).unwrap();
            alloc.committed(txn);
        }
        let mut sample = [0; BLOCK];
        disk.read(head.addr, &mut sample).unwrap();
        let mut rng = XorShift::new(txns);
        for _ in 0..5_000 {
            let block = block_of(&hostile::mutate(&mut rng, &sample));
            let mut copy = disk.clone();
            copy.write(head.addr, &block).unwrap();
            let ptr = BlockPtr {
                csum: kind.sum(&block),
                ..head
            };
            if let Ok(loaded) = Allocator::load(&mut copy, kind, &ptr, txns + rng.below(3) as u64) {
                let state = loaded.state();
                assert!(
                    state
                        .free
                        .iter()
                        .all(|&(s, l)| s >= area.0 && s + l <= area.1)
                );
                assert!(
                    state
                        .pending
                        .iter()
                        .all(|&(_, s, l)| s >= area.0 && s + l <= area.1)
                );
            }
        }
    }
}

fn node_is_total(bytes: &[u8]) {
    use kaifs::tree::Node;
    if let Ok(node) = Node::decode(&block_of(bytes)) {
        assert_eq!(Node::decode(&node.encode()), Ok(node));
    }
}

/// Mutated leaves and branches, and every field at every extreme value.
#[test]
fn mutated_nodes_never_panic_and_what_decodes_round_trips() {
    use kaifs::tree::{Child, Key, Node};
    let leaf = Node::Leaf(
        (0..40u64)
            .map(|i| {
                (
                    Key::new(i, (i % 3) as u8, i * 7),
                    vec![i as u8; (i * 13 % 90) as usize],
                )
            })
            .collect(),
    );
    let child = BlockPtr {
        addr: 900,
        birth: 4,
        blocks: 1,
        kind: KIND_NODE,
        level: 1,
        flags: 0,
        csum: [3; 32],
    };
    let branch = Node::Branch(
        2,
        (0..30u64)
            .map(|i| {
                (
                    Key::new(i * 100, 1, 0),
                    Child::Clean(BlockPtr {
                        addr: 900 + i,
                        ..child
                    }),
                )
            })
            .collect(),
    );
    for (seed, sample) in [(5, leaf.encode()), (6, branch.encode())] {
        fuzz(&sample, seed, 20_000, node_is_total);
        sweep_fields(&sample, 0..200, |_, bytes| node_is_total(bytes));
    }
}

/// Each namespace item reader on mutated samples and every field at every
/// extreme value: decode or refuse, never panic, and what decodes round-trips.
#[test]
fn mutated_namespace_items_never_panic_and_what_decodes_round_trips() {
    use kaifs::FileType;
    use kaifs::items::{Entry, Inode, decode_attrs, decode_names, encode_attrs, encode_names};
    let inode = Inode {
        kind: FileType::File,
        mode: 0o644,
        nlink: 1,
        size: 300_000,
        parent: 7,
        mtime: 11,
        ctime: 12,
        crtime: 10,
        changed: 13,
    }
    .encode();
    let inode_is_total = |bytes: &[u8]| {
        if let Ok(inode) = Inode::decode(bytes) {
            assert_eq!(Inode::decode(&inode.encode()), Ok(inode));
        }
    };
    fuzz(&inode, 7, 20_000, inode_is_total);
    sweep_fields(&inode, 0..inode.len(), |_, bytes| inode_is_total(bytes));
    let entries: Vec<Entry> = ["a", "bb", "\u{65e5}\u{8a18}", "zzz"]
        .iter()
        .enumerate()
        .map(|(i, n)| Entry {
            name: n.to_string(),
            ino: 2 + i as u64,
            kind: [FileType::File, FileType::Dir][i % 2],
        })
        .collect();
    let names = encode_names(&entries);
    let names_are_total = |bytes: &[u8]| {
        if let Ok(decoded) = decode_names(bytes) {
            assert_eq!(encode_names(&decoded), bytes);
        }
    };
    fuzz(&names, 8, 20_000, names_are_total);
    sweep_fields(&names, 0..names.len(), |_, bytes| names_are_total(bytes));
    let attrs = encode_attrs(&[
        ("user.a".to_string(), b"one".to_vec()),
        ("user.b".to_string(), Vec::new()),
        ("user.c".to_string(), vec![0xAB; 40]),
    ]);
    let attrs_are_total = |bytes: &[u8]| {
        if let Ok(decoded) = decode_attrs(bytes) {
            assert_eq!(encode_attrs(&decoded), bytes);
        }
    };
    fuzz(&attrs, 9, 20_000, attrs_are_total);
    sweep_fields(&attrs, 0..attrs.len(), |_, bytes| attrs_are_total(bytes));
}

/// Walks what a namespace shows a reader, never trusting it: every name, every
/// inode, a bounded read of every file and every attribute.
fn browse(fs: &mut kaifs::Fs<MemDisk>) {
    use kaifs::{FileType, ROOT};
    let mut seen = std::collections::BTreeSet::new();
    let mut stack = vec![ROOT];
    while let Some(dir) = stack.pop() {
        if !seen.insert(dir) || seen.len() > 2_000 {
            continue;
        }
        let _ = fs.xattrs(dir);
        let Ok(names) = fs.list(dir, None, 500) else {
            continue;
        };
        for listed in names {
            let ino = listed.entry.ino;
            let _ = fs.lookup(dir, &listed.entry.name);
            match fs.stat(ino).map(|i| i.kind) {
                Ok(FileType::Dir) => stack.push(ino),
                Ok(FileType::File) => {
                    let _ = fs.read_at(ino, 0, 1 << 20);
                    let _ = fs.read_at(ino, u64::MAX - 10, 100);
                    let _ = fs.xattrs(ino);
                }
                Err(_) => {}
            }
        }
    }
}

/// A sound namespace whose items are replaced by mutated bytes, or joined by
/// items at random keys: reads, check and the namespace rules must never panic,
/// and repair must refuse or leave a namespace that passes check.
#[test]
fn a_namespace_with_hostile_items_never_panics_and_repairs_to_one_that_checks() {
    use kaifs::check::{Cache, check};
    use kaifs::items::{K_DATA, K_DIRENT, K_INODE, K_XATTR};
    use kaifs::ns_check::check_namespace;
    use kaifs::repair::repair;
    use kaifs::tree::Key;
    use kaifs::{Fs, ROOT};
    let mut fs = Fs::mkfs(MemDisk::new(4096), [5; 16], "hostile").unwrap();
    fs.hash_mask = 0xF;
    let d = fs.mkdir(ROOT, "d").unwrap();
    for i in 0..30u64 {
        let f = fs
            .create(if i % 2 == 0 { ROOT } else { d }, &format!("f{i}"))
            .unwrap();
        fs.write_file(f, &vec![i as u8 + 1; (i as usize * 97) % 3000])
            .unwrap();
        if i % 5 == 0 {
            fs.set_xattr(f, "user.x", &[i as u8; 9]).unwrap();
        }
    }
    fs.commit().unwrap();
    let mut items: Vec<(Key, Vec<u8>)> = Vec::new();
    fs.scan_raw(&Key::MIN, &mut |k, v| {
        items.push((*k, v.to_vec()));
        true
    })
    .unwrap();
    let base = fs.into_dev();
    let cases = if cfg!(debug_assertions) { 150 } else { 1_500 };
    let mut rng = XorShift::new(10);
    let mut repaired = 0;
    for _ in 0..cases {
        let mut fs = Fs::mount(base.clone()).unwrap();
        fs.hash_mask = 0xF;
        for _ in 0..1 + rng.below(3) {
            let (key, raw) = &items[rng.below(items.len())];
            let inline = kaifs::fs::inline_of(raw).unwrap_or(&raw[1..]);
            let bytes = hostile::mutate(&mut rng, inline);
            let key = match rng.below(4) {
                0 => *key,
                1 => Key::new(key.oid, key.kind, rng.next() % 64),
                2 => Key::new(
                    rng.below(70) as u64,
                    [K_INODE, K_DIRENT, K_DATA, K_XATTR, 9][rng.below(5)],
                    key.off,
                ),
                _ => Key::new(key.oid + rng.below(3) as u64, key.kind, key.off),
            };
            if bytes.len() <= kaifs::fs::MAX_INLINE {
                let _ = fs.put(key, &bytes);
            } else {
                let _ = fs.delete(&key);
            }
        }
        fs.commit().unwrap();
        browse(&mut fs);
        let mut cache = Cache::default();
        let _ = check(&mut fs, &mut cache);
        let _ = check_namespace(&mut fs, &mut cache);
        if let Ok(fixes) = repair(&mut fs) {
            repaired += usize::from(!fixes.is_empty());
            let mut cache = Cache::default();
            check(&mut fs, &mut cache).unwrap();
            check_namespace(&mut fs, &mut cache)
                .unwrap_or_else(|e| panic!("after repair: {e}\n{fixes:#?}"));
            browse(&mut fs);
        }
    }
    assert!(
        repaired > cases / 2,
        "only {repaired} of {cases} cases needed a repair"
    );
}

/// The volume table's readers (task 6): volume and snapshot entries, and UNDO
/// records split over parts. Each decodes or is refused, never panics, and what
/// decodes encodes back to itself.
#[test]
fn mutated_volume_table_entries_never_panic_and_what_decodes_round_trips() {
    use kaifs::vol::{
        Snap, SnapKind, Undo, VolEntry, decode_snapshot, decode_undo, decode_volume,
        encode_snapshot, encode_undo, encode_volume,
    };
    let root = BlockPtr {
        kind: KIND_NODE,
        blocks: 1,
        level: 2,
        flags: 0,
        ..sample_ptr()
    };
    let snap = Snap {
        root,
        generation: 41,
        next_oid: 900,
        kind: SnapKind::Undo,
        expiry: 1 << 60,
        pin: 77,
    };
    let volume = encode_volume(&VolEntry {
        live: snap,
        name: "home".to_string(),
        key_id: 0,
    });
    fuzz(&volume, 11, 20_000, |bytes| {
        if let Ok(entry) = decode_volume(2, bytes) {
            assert_eq!(decode_volume(2, &encode_volume(&entry)), Ok(entry));
        }
    });
    sweep_fields(&volume, 0..volume.len(), |_, bytes| {
        let _ = decode_volume(2, bytes);
    });
    let snapshot = encode_snapshot(&snap);
    fuzz(&snapshot, 12, 20_000, |bytes| {
        if let Ok(s) = decode_snapshot(bytes) {
            assert_eq!(decode_snapshot(&encode_snapshot(&s)), Ok(s));
        }
    });
    let undo = Undo {
        txn: 50,
        before: 41,
        action: 77,
        time: 123,
        snapshot: 3,
        objects: (1..400).collect(),
    };
    let parts = encode_undo(&undo).unwrap();
    assert!(parts.len() > 1);
    assert_eq!(decode_undo(50, &parts), Ok(undo));
    let mut rng = XorShift::new(13);
    for _ in 0..20_000 {
        let mut mutated = parts.clone();
        let at = rng.below(mutated.len());
        mutated[at] = hostile::mutate(&mut rng, &mutated[at]);
        if rng.below(8) == 0 {
            mutated.truncate(rng.below(mutated.len()));
        }
        // Parts may split anywhere on an object, so a record that decodes need
        // not encode to the same parts; it must only not panic.
        let _ = decode_undo(50, &mutated);
    }
}

/// `info.cbor`'s reader, on mutated samples, every field at every extreme value,
/// and nests of arrays and maps that claim more than they hold: it decodes or
/// refuses, never panics, never allocates past the input, and what decodes
/// encodes back to the same record.
#[test]
fn mutated_info_cbor_never_panics_and_what_decodes_round_trips() {
    use kaifs::trash::{TrashInfo, decode_info, encode_info};
    let sample = encode_info(&TrashInfo {
        path: "/user/photos/2026/\u{65e5}\u{8a18}.jpg".to_string(),
        deleted: 1_792_972_800_000_000_000,
        action: 4_242,
    });
    let total = |bytes: &[u8]| {
        if let Ok(info) = decode_info(bytes) {
            assert_eq!(decode_info(&encode_info(&info)), Ok(info));
        }
    };
    fuzz(&sample, 14, 50_000, total);
    sweep_fields(&sample, 0..sample.len(), |_, bytes| total(bytes));
    let mut nests = vec![0x82, 0x01];
    nests.extend(std::iter::repeat_n(0x9B, 30));
    nests.extend_from_slice(&[0xFF; 8]);
    total(&nests);
    let mut maps = vec![0x82, 0x01, 0xBB];
    maps.extend_from_slice(&[0x7F; 8]);
    total(&maps);
}
