//! Hostile images that lie about births and blocks across trees (the #1527
//! review's LOWs): a generation at the cap, a snapshot id at the top, a
//! snapshot that disagrees with its volume's other trees, and a tree that
//! reaches another structure's block. Each is signed again as in
//! `hostile_images.rs`, so the lie reaches the readers behind the checksums.

#[path = "hostile/map.rs"]
mod map;

use kaifs::check::{Cache, Report, check, tree_runs};
use kaifs::format::{self, BlockPtr, KIND_RECORD, MAX_GENERATION, Superblock};
use kaifs::fs::{HOME, SYSTEM, record_of};
use kaifs::ns_check::check_namespace;
use kaifs::repair_fs::{mount_to_repair, repair_fs};
use kaifs::tree::{Child, Key, Node};
use kaifs::vol::{K_SNAPSHOT, SnapKind};
use kaifs::{BlockDev, Error, Fs, MemDisk, ROOT};
use map::{Holder, Map, block};
use std::collections::BTreeSet;
use std::rc::Rc;

const BLOCKS: u64 = 2048;

/// The snapshots and action [`history`] took.
struct Ids {
    /// Named, before file-000 changed; then named again after it.
    a: u64,
    b: u64,
    /// The action's undo snapshot, and its commit.
    undo: u64,
    txn: u64,
}

/// A file in system, and in home: inline files, two in records, a named
/// snapshot, a change, another named snapshot, a change, and an action that
/// writes `rec` again.
fn history() -> (MemDisk, Ids) {
    let mut fs = Fs::mkfs(MemDisk::new(BLOCKS), [6; 16], "history").unwrap();
    fs.set_volume(SYSTEM).unwrap();
    let s = fs.create(ROOT, "s").unwrap();
    fs.write_file(s, &[3; 3000]).unwrap();
    fs.commit().unwrap();
    fs.set_volume(HOME).unwrap();
    let dir = fs.mkdir(ROOT, "d").unwrap();
    for n in 0..60u32 {
        let f = fs.create(dir, &format!("file-{n:03}")).unwrap();
        fs.write_file(f, &[n as u8; 300]).unwrap();
    }
    let file = |fs: &mut Fs<MemDisk>, name: &str| fs.lookup(dir, name).unwrap().unwrap().ino;
    let f0 = file(&mut fs, "file-000");
    fs.write_file(f0, &[1; 3000]).unwrap();
    let rec = fs.create(dir, "rec").unwrap();
    fs.write_file(rec, &[2; 3000]).unwrap();
    fs.commit().unwrap();
    let a = fs.snapshot_as(SnapKind::Named, 0, 0).unwrap();
    fs.commit().unwrap();
    fs.write_file(f0, b"changed after a").unwrap();
    fs.commit().unwrap();
    let b = fs.snapshot_as(SnapKind::Named, 0, 0).unwrap();
    fs.commit().unwrap();
    let f1 = file(&mut fs, "file-001");
    fs.write_file(f1, b"changed after b").unwrap();
    fs.commit().unwrap();
    fs.begin(1).unwrap();
    fs.write_file(rec, &[4; 3000]).unwrap();
    let txn = fs.commit().unwrap();
    let undo = *fs
        .snapshots()
        .iter()
        .find(|(_, s)| s.kind == SnapKind::Undo)
        .unwrap()
        .0;
    (fs.into_dev(), Ids { a, b, undo, txn })
}

fn check_of(disk: MemDisk) -> Result<Report, String> {
    let mut fs = Fs::mount(disk).unwrap();
    check(&mut fs, &mut Cache::default())
}

/// Home's snapshots, by id, with their digests.
fn snaps(report: &Report) -> Vec<(u64, u64)> {
    let home = &report.volumes[&HOME].snaps;
    home.iter().map(|(id, t)| (*id, t.digest)).collect()
}

/// The addresses the tree `root` reaches.
fn reached(fs: &mut Fs<MemDisk>, root: BlockPtr) -> BTreeSet<u64> {
    let runs = tree_runs(&mut fs.io, &root, &mut Cache::default()).unwrap();
    runs.iter().map(|h| h.run.addr).collect()
}

/// Repairs `disk`, which `check` refuses; after, it checks, home keeps the
/// snapshots of `base` but `dropped`, as they read there, and writes over home
/// commit and check again. Returns repair's lines.
fn repairs(disk: MemDisk, base: &MemDisk, dropped: &[u64]) -> Vec<String> {
    let mut want = snaps(&check_of(base.clone()).unwrap());
    want.retain(|(id, _)| !dropped.contains(id));
    let (mut fs, _) = mount_to_repair(disk).unwrap();
    let lines = repair_fs(&mut fs).unwrap();
    assert!(!lines.is_empty());
    let mut fs = Fs::mount(fs.into_dev()).unwrap();
    for round in 0..2u8 {
        let mut cache = Cache::default();
        let report = check(&mut fs, &mut cache)
            .unwrap_or_else(|e| panic!("round {round}: {e} after {lines:#?}"));
        check_namespace(&mut fs, &mut cache).unwrap();
        assert_eq!(snaps(&report), want, "after {lines:#?}");
        fs.set_volume(HOME).unwrap();
        let dir = fs.lookup(ROOT, "d").unwrap().unwrap().ino;
        for name in ["rec", "file-000", "file-059"] {
            let f = fs.lookup(dir, name).unwrap().unwrap().ino;
            fs.write_file(f, &[round + 9; 5000]).unwrap();
        }
        fs.commit().unwrap();
    }
    lines
}

fn has(lines: &[String], want: &str) -> bool {
    lines.iter().any(|l| l.contains(want))
}

/// A slot signed again at `MAX_GENERATION` mounts, but its next commit would
/// write a slot the mount after reads as damaged: commit refuses, and the disk
/// keeps what it had.
#[test]
fn a_commit_past_the_highest_generation_is_refused() {
    let (base, _) = history();
    let mut disk = base.clone();
    let newest = format::read_rings(&mut disk).unwrap()[0];
    let mut capped = newest;
    capped.generation = MAX_GENERATION;
    for slot in format::all_slots(disk.info().blocks) {
        let current = Superblock::decode(&block(&disk, slot));
        if current.is_ok_and(|sb| sb.generation == newest.generation) {
            disk.put(slot, Rc::new(capped.encode()));
        }
    }
    let mut fs = Fs::mount(disk).unwrap();
    fs.set_volume(HOME).unwrap();
    let f = fs.create(ROOT, "lost").unwrap();
    fs.write_file(f, b"never on the disk").unwrap();
    let refused = fs.commit().unwrap_err();
    assert!(matches!(refused, Error::Invalid(_)), "{refused:?}");
    let mut fs = Fs::mount(fs.into_dev()).unwrap();
    assert_eq!(fs.committed, MAX_GENERATION);
    fs.set_volume(HOME).unwrap();
    assert!(fs.lookup(ROOT, "lost").unwrap().is_none());
    // A mount for repair refuses the same way.
    let (mut fs, _) = mount_to_repair(fs.into_dev()).unwrap();
    fs.set_volume(HOME).unwrap();
    fs.create(ROOT, "lost").unwrap();
    assert!(matches!(fs.commit(), Err(Error::Invalid(_))));
}

/// A snapshot id at `u64::MAX` leaves no id for the next: mount refuses it as
/// corrupt, where it overflowed.
#[test]
fn a_snapshot_id_at_the_top_is_refused_at_mount() {
    let (base, ids) = history();
    let mut map = Map::of(&mut base.clone());
    let mut disk = base.clone();
    // The table leaf holding the undo snapshot, home's last.
    let mut fs = Fs::mount(base.clone()).unwrap();
    fs.set_volume(HOME).unwrap();
    let root = fs.snapshots()[&ids.undo].root;
    let Holder::Entry { node: leaf, .. } = map.blocks[&root.addr].0 else {
        panic!("the undo snapshot's root is its own");
    };
    let Ok(Node::Leaf(mut items)) = Node::decode(&block(&disk, leaf)) else {
        panic!("a volume table leaf");
    };
    // Home's last snapshot: the next item is of another kind or volume, so the
    // leaf stays in order.
    let last = items
        .iter()
        .rposition(|(k, _)| k.oid == HOME && k.kind == K_SNAPSHOT)
        .unwrap();
    items[last].0 = Key::new(HOME, K_SNAPSHOT, u64::MAX);
    disk.put(leaf, Rc::new(Node::Leaf(items).encode()));
    map.sign(&mut disk, leaf);
    match Fs::mount(disk) {
        Err(Error::Corrupt(why)) => assert!(why.contains("snapshot id"), "{why}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

/// Rule 7 on two snapshots that give one block two births: the earlier lies,
/// the later and the live tree agree. The birth two trees give wins, so the
/// liar is dropped and the honest one kept; keeping the earlier would have
/// dropped the honest one.
#[test]
fn a_snapshot_its_volume_outvotes_on_a_birth_is_dropped() {
    let (base, ids) = history();
    let map = Map::of(&mut base.clone());
    let mut fs = Fs::mount(base.clone()).unwrap();
    fs.set_volume(HOME).unwrap();
    let root = |fs: &Fs<MemDisk>, id: u64| fs.snapshots()[&id].root;
    let (ra, rb) = (root(&fs, ids.a), root(&fs, ids.b));
    let live = fs.live_root().unwrap();
    let (a, b, live) = (
        reached(&mut fs, ra),
        reached(&mut fs, rb),
        reached(&mut fs, live),
    );
    let leaf_without_records = |ptr: &BlockPtr| match Node::decode(&block(&base, ptr.addr)) {
        Ok(Node::Leaf(items)) => items.iter().all(|(_, raw)| record_of(raw).is_none()),
        _ => false,
    };
    // A node only a reaches, over a leaf all three share.
    let (holder, ptr) = a
        .iter()
        .filter(|n| !b.contains(n) && !live.contains(n))
        .find_map(|&node| {
            let Ok(Node::Branch(_, children)) = Node::decode(&block(&base, node)) else {
                return None;
            };
            children.iter().enumerate().find_map(|(index, (_, c))| {
                let Child::Clean(c) = c else { return None };
                let shared = b.contains(&c.addr) && live.contains(&c.addr);
                (shared && c.birth > 1 && leaf_without_records(c))
                    .then_some((Holder::Child { node, index }, *c))
            })
        })
        .expect("a branch of a over a leaf every tree shares");
    let mut disk = base.clone();
    map.clone()
        .point(&mut disk, holder, BlockPtr { birth: 1, ..ptr });
    let refused = check_of(disk.clone()).unwrap_err();
    assert!(refused.contains("reached with two births"), "{refused}");
    let lines = repairs(disk, &base, &[ids.a]);
    let dropped = format!("snapshot {} does not check (block {}", ids.a, ptr.addr);
    assert!(has(&lines, &dropped), "{lines:#?}");
    assert!(!has(&lines, &format!("snapshot {} ", ids.b)), "{lines:#?}");
}

/// Rule 7 on an undo snapshot whose root is newer than it: dropped with the
/// UNDO record it backs, and the rest checks.
#[test]
fn an_undo_snapshot_rooted_after_it_is_dropped_with_its_record() {
    let (base, ids) = history();
    let map = Map::of(&mut base.clone());
    let mut fs = Fs::mount(base.clone()).unwrap();
    fs.set_volume(HOME).unwrap();
    assert_eq!(fs.undo_records().unwrap().len(), 1);
    let snap = fs.snapshots()[&ids.undo];
    let (holder, ..) = map.blocks[&snap.root.addr];
    assert!(matches!(holder, Holder::Entry { .. }), "{holder:?}");
    let mut disk = base.clone();
    let birth = snap.generation + 1;
    map.clone()
        .point(&mut disk, holder, BlockPtr { birth, ..snap.root });
    let refused = check_of(disk.clone()).unwrap_err();
    assert!(refused.contains("a tree root newer"), "{refused}");
    let (mut fs, _) = mount_to_repair(disk.clone()).unwrap();
    let lines = repair_fs(&mut fs).unwrap();
    let why = format!("snapshot {} does not check (its root is newer", ids.undo);
    assert!(has(&lines, &why), "{lines:#?}");
    let record = format!("the undo record of {} does not hold: dropped", ids.txn);
    assert!(has(&lines, &record), "{lines:#?}");
    fs.set_volume(HOME).unwrap();
    assert!(fs.undo_records().unwrap().is_empty());
    repairs(disk, &base, &[ids.undo]);
}

/// A record home reaches that is another structure's block, at another birth:
/// system's record, or the allocator's head. `check` refuses two births;
/// repair copies the record in the live tree, and drops a snapshot that
/// reaches one. (The volume table's blocks cannot be named so here: the table
/// holds home's roots, so signing the lie again rewrites the block it names.)
#[test]
fn a_tree_reaching_another_structures_block_is_rebuilt_or_dropped() {
    let (base, ids) = history();
    let map = Map::of(&mut base.clone());
    let mut fs = Fs::mount(base.clone()).unwrap();
    let system = fs.live_root_of(SYSTEM).unwrap();
    let system = reached(&mut fs, system);
    fs.set_volume(HOME).unwrap();
    let roots: Vec<(u64, BlockPtr)> = fs.snapshots().iter().map(|(id, s)| (*id, s.root)).collect();
    let (mut a, mut others) = (BTreeSet::new(), BTreeSet::new());
    for (id, root) in roots {
        match id == ids.a {
            true => a = reached(&mut fs, root),
            false => others.extend(reached(&mut fs, root)),
        }
    }
    let root = fs.live_root().unwrap();
    let live = reached(&mut fs, root);
    // A record only `only` reaches, in a leaf only it reaches.
    let record_in = |only: &BTreeSet<u64>, rest: &BTreeSet<u64>| {
        map.blocks
            .values()
            .find(|(h, p, _)| match h {
                Holder::Record { node, .. } => {
                    only.contains(node) && !rest.contains(node) && !rest.contains(&p.addr)
                }
                _ => false,
            })
            .map(|&(h, p, _)| (h, p, map.blocks[&holder_node(h)].1.birth))
            .unwrap()
    };
    let mut not_live = others.clone();
    not_live.extend(a.iter().copied());
    let mut not_a = others.clone();
    not_a.extend(live.iter().copied());
    let system_record = map
        .blocks
        .values()
        .find(|(h, p, _)| {
            matches!(h, Holder::Record { node, .. } if system.contains(node)) && p.blocks == 1
        })
        .map(|(_, p, _)| *p)
        .unwrap();
    let targets = [
        ("in volume 1", system_record),
        ("in the allocator's chain", map.sb.alloc),
    ];
    for (whose, target) in targets {
        for (tree, rest) in [("live", &not_live), ("a", &not_a)] {
            let only = match tree {
                "live" => &live,
                _ => &a,
            };
            let (holder, record, leaf_birth) = record_in(only, rest);
            let birth = leaf_birth.min(target.birth - 1).max(1);
            assert_ne!(birth, target.birth);
            let lie = BlockPtr {
                birth,
                blocks: 1,
                kind: KIND_RECORD,
                level: 0,
                flags: record.flags,
                ..target
            };
            let mut disk = base.clone();
            map.clone().point(&mut disk, holder, lie);
            let refused = check_of(disk.clone()).unwrap_err();
            assert!(refused.contains("two births"), "{whose}, {tree}: {refused}");
            let dropped: &[u64] = match tree {
                "live" => &[],
                _ => &[ids.a],
            };
            let lines = repairs(disk, &base, dropped);
            let why = format!("(block {} is also {whose})", target.addr);
            let want = match tree {
                "live" => format!("the live tree does not check {why}"),
                _ => format!("snapshot {} does not check {why}", ids.a),
            };
            assert!(has(&lines, &want), "{whose}, {tree}: {lines:#?}");
        }
    }
}

fn holder_node(holder: Holder) -> u64 {
    match holder {
        Holder::Record { node, .. } | Holder::Child { node, .. } | Holder::Entry { node, .. } => {
            node
        }
        _ => unreachable!(),
    }
}
