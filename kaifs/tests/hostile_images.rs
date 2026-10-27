//! Hostile whole images (kaifs hardening; disks become untrusted input in P7).
//! A block the newest commit reaches (a node, an allocator block or a record)
//! is changed and then signed again: the checksum in its pointer, and in every
//! parent's up to the superblock, is rewritten, so the change reaches the
//! parsers behind the checksums. Every reader must answer or refuse, never
//! panic, and read a number of nodes bounded by the disk's size: a hostile tree
//! that names one node under every entry, at every level, would otherwise cost
//! a number of reads that grows with its depth as a power.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use hostile::XorShift;
use kaifs::check::{Cache, check};
use kaifs::format::{self, BlockPtr, KIND_NODE, PTR_BYTES, Superblock};
use kaifs::fs::{HOME, SYSTEM, record_of};
use kaifs::ns_check::check_namespace;
use kaifs::repair::repair;
use kaifs::tree::{Child, Key, Node};
use kaifs::vol::{K_SNAPSHOT, K_VOLUME, decode_snapshot, decode_volume, encode_snapshot};
use kaifs::vol::{SnapKind, encode_volume};
use kaifs::{BLOCK, Block, BlockDev, CsumKind, FileType, Fs, MemDisk, ROOT};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

const BLOCKS: u64 = 2048;

/// Where the pointer to a block lies.
#[derive(Clone, Copy, Debug)]
enum Holder {
    /// The superblock's volume-table root, or its allocator head.
    Volumes,
    Alloc,
    /// Child `index` of the branch at `node`.
    Child {
        node: u64,
        index: usize,
    },
    /// Item `index` of the volume-table leaf at `node`: a live or snapshot root.
    Entry {
        node: u64,
        index: usize,
    },
    /// Item `index` of the leaf at `node`: a DATA item's record.
    Record {
        node: u64,
        index: usize,
    },
    /// The allocator block at `block`, whose next pointer it is.
    Prev {
        block: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum What {
    Node,
    Alloc,
    Record,
}

/// Every block the newest commit reaches, with the first place found that
/// points to it.
struct Map {
    sb: Superblock,
    blocks: BTreeMap<u64, (Holder, BlockPtr, What)>,
}

fn block(disk: &MemDisk, addr: u64) -> Block {
    **disk.get(addr).expect("a block the image holds")
}

impl Map {
    fn of(disk: &mut MemDisk) -> Map {
        let sb = format::read_rings(disk).unwrap()[0];
        let mut map = Map {
            sb,
            blocks: BTreeMap::new(),
        };
        map.tree(disk, Holder::Volumes, sb.volumes, true);
        let (mut holder, mut ptr) = (Holder::Alloc, sb.alloc);
        while !ptr.is_null() && !map.blocks.contains_key(&ptr.addr) {
            map.blocks.insert(ptr.addr, (holder, ptr, What::Alloc));
            let next = BlockPtr::decode_at(&block(disk, ptr.addr), 16).unwrap();
            (holder, ptr) = (Holder::Prev { block: ptr.addr }, next);
        }
        map
    }

    fn tree(&mut self, disk: &MemDisk, holder: Holder, ptr: BlockPtr, table: bool) {
        if ptr.is_null() || self.blocks.contains_key(&ptr.addr) {
            return;
        }
        self.blocks.insert(ptr.addr, (holder, ptr, What::Node));
        match Node::decode(&block(disk, ptr.addr)).unwrap() {
            Node::Branch(_, children) => {
                for (index, (_, child)) in children.into_iter().enumerate() {
                    let Child::Clean(child) = child else {
                        unreachable!("a node read from disk is clean");
                    };
                    let at = Holder::Child {
                        node: ptr.addr,
                        index,
                    };
                    self.tree(disk, at, child, table);
                }
            }
            Node::Leaf(items) => {
                for (index, (key, raw)) in items.into_iter().enumerate() {
                    let node = ptr.addr;
                    if table && key.kind == K_VOLUME {
                        let root = decode_volume(key.oid, &raw).unwrap().live.root;
                        self.tree(disk, Holder::Entry { node, index }, root, false);
                    } else if table && key.kind == K_SNAPSHOT {
                        let root = decode_snapshot(&raw).unwrap().root;
                        self.tree(disk, Holder::Entry { node, index }, root, false);
                    } else if let Some((record, _)) = record_of(&raw).filter(|_| !table) {
                        let at = Holder::Record { node, index };
                        self.blocks
                            .entry(record.addr)
                            .or_insert((at, record, What::Record));
                    }
                }
            }
        }
    }

    /// Signs the block at `addr` again after a change, and every block up to
    /// the superblock.
    fn sign(&mut self, disk: &mut MemDisk, addr: u64) {
        let (holder, ptr, _) = self.blocks[&addr];
        let mut bytes = Vec::with_capacity(ptr.blocks as usize * BLOCK);
        for i in 0..u64::from(ptr.blocks) {
            bytes.extend_from_slice(&block(disk, ptr.addr + i));
        }
        let ptr = BlockPtr {
            csum: CsumKind::Seahash64.sum(&bytes),
            ..ptr
        };
        self.point(disk, holder, ptr);
    }

    /// Makes `holder` name `ptr`, and signs its block again.
    fn point(&mut self, disk: &mut MemDisk, holder: Holder, ptr: BlockPtr) {
        let edit = |disk: &mut MemDisk, at: u64, f: &mut dyn FnMut(&mut Node)| {
            let mut node = Node::decode(&block(disk, at)).unwrap();
            f(&mut node);
            disk.put(at, Rc::new(node.encode()));
        };
        let leaf_item = |node: &mut Node, index: usize| match node {
            Node::Leaf(items) => (items[index].0, std::mem::take(&mut items[index].1)),
            Node::Branch(..) => unreachable!("an item of a leaf"),
        };
        let up = match holder {
            Holder::Volumes | Holder::Alloc => {
                match holder {
                    Holder::Volumes => self.sb.volumes = ptr,
                    _ => self.sb.alloc = ptr,
                }
                let generation = self.sb.generation;
                for slot in format::all_slots(disk.info().blocks) {
                    let current = Superblock::decode(&block(disk, slot));
                    if current.is_ok_and(|sb| sb.generation == generation) {
                        disk.put(slot, Rc::new(self.sb.encode()));
                    }
                }
                return;
            }
            Holder::Child { node, index } => {
                edit(disk, node, &mut |n| {
                    if let Node::Branch(_, children) = n {
                        children[index].1 = Child::Clean(ptr);
                    }
                });
                node
            }
            Holder::Entry { node, index } => {
                edit(disk, node, &mut |n| {
                    let (key, raw) = leaf_item(n, index);
                    let raw = match key.kind {
                        K_VOLUME => {
                            let mut entry = decode_volume(key.oid, &raw).unwrap();
                            entry.live.root = ptr;
                            encode_volume(&entry)
                        }
                        _ => {
                            let mut snap = decode_snapshot(&raw).unwrap();
                            snap.root = ptr;
                            encode_snapshot(&snap)
                        }
                    };
                    if let Node::Leaf(items) = n {
                        items[index].1 = raw;
                    }
                });
                node
            }
            Holder::Record { node, index } => {
                edit(disk, node, &mut |n| {
                    if let Node::Leaf(items) = n {
                        items[index].1[1..1 + PTR_BYTES].copy_from_slice(&ptr.encode());
                    }
                });
                node
            }
            Holder::Prev { block: at } => {
                let mut bytes = block(disk, at);
                bytes[16..16 + PTR_BYTES].copy_from_slice(&ptr.encode());
                disk.put(at, Rc::new(bytes));
                at
            }
        };
        self.sign(disk, up);
    }
}

/// A disk with history in two volumes: folders, inline and record data,
/// attributes, actions with their undo snapshots, the trash, and a named
/// snapshot.
fn sample() -> MemDisk {
    let mut fs = Fs::mkfs(MemDisk::new(BLOCKS), [9; 16], "hostile").unwrap();
    for n in 0..24u64 {
        fs.begin(n + 1).unwrap();
        fs.set_volume([HOME, SYSTEM][n as usize % 2]).unwrap();
        let dir = match fs.lookup(ROOT, "d").unwrap() {
            Some(e) => e.ino,
            None => fs.mkdir(ROOT, "d").unwrap(),
        };
        let f = fs.create(dir, &format!("f{n}")).unwrap();
        fs.write_file(f, &vec![n as u8 + 1; 100 + (n as usize * 911) % 6000])
            .unwrap();
        if n % 5 == 0 {
            fs.set_xattr(f, "user.n", &[n as u8; 12]).unwrap();
        }
        if n % 7 == 6 {
            fs.trash(dir, &format!("f{}", n - 2)).unwrap();
        }
        fs.commit().unwrap();
    }
    fs.set_volume(HOME).unwrap();
    fs.snapshot_as(SnapKind::Named, 0, 0).unwrap();
    fs.commit().unwrap();
    fs.into_dev()
}

/// Every reader on `disk`, whatever it holds; returns the nodes read.
fn battery(disk: MemDisk) -> u64 {
    let Ok(mut fs) = Fs::mount(disk) else {
        return 0;
    };
    let mut cache = Cache::default();
    let _ = check(&mut fs, &mut cache);
    let _ = check_namespace(&mut fs, &mut cache);
    for (id, _) in fs.volumes() {
        if fs.set_volume(id).is_err() {
            continue;
        }
        let mut stack = vec![ROOT];
        let mut seen = BTreeSet::new();
        while let Some(dir) = stack.pop() {
            if !seen.insert(dir) || seen.len() > 500 {
                continue;
            }
            let _ = fs.xattrs(dir);
            let _ = fs.path(None, dir);
            let Ok(names) = fs.list(dir, None, 200) else {
                continue;
            };
            for listed in names {
                let ino = listed.entry.ino;
                match fs.stat(ino).map(|i| i.kind) {
                    Ok(FileType::Dir) => stack.push(ino),
                    Ok(FileType::File) => {
                        let _ = fs.read_at(ino, 0, 1 << 16);
                        let _ = fs.history(ino);
                    }
                    Err(_) => {}
                }
            }
        }
        let _ = fs.undo_records();
        let _ = fs.trash_list();
        let _ = fs.changes(0);
    }
    // A writer on it, and repair, must refuse or go on too.
    if fs.set_volume(HOME).is_ok() {
        let _ = fs
            .create(ROOT, "after")
            .and_then(|f| fs.write_file(f, b"later"));
        let _ = fs.commit();
    }
    for (id, _) in fs.volumes() {
        if fs.set_volume(id).is_ok() {
            let _ = repair(&mut fs);
        }
    }
    fs.io.node_reads
}

/// The node at `addr`, changed by one of a hostile disk's moves.
fn changed_node(rng: &mut XorShift, node: Node) -> Node {
    match node {
        Node::Branch(level, mut children) => {
            let n = children.len();
            match rng.below(3) {
                // One child named twice, or under every entry.
                0 => children[rng.below(n)].1 = children[rng.below(n)].1.clone(),
                1 => {
                    let first = children[0].1.clone();
                    children.iter_mut().for_each(|c| c.1 = first.clone());
                }
                _ => children[rng.below(n)].0 = Key::new(rng.next() % 64, 1, rng.next() % 8),
            }
            Node::Branch(level, children)
        }
        Node::Leaf(mut items) => {
            match (rng.below(3), items.len()) {
                (0, _) | (_, 0) => items.clear(),
                (1, n) => items[rng.below(n)].0 = Key::new(rng.next() % 64, 1, rng.next() % 8),
                (_, n) => {
                    let value = &mut items[rng.below(n)].1;
                    let len = value.len();
                    let mut bytes = hostile::mutate(rng, value);
                    bytes.resize(len, 0);
                    *value = bytes;
                }
            }
            Node::Leaf(items)
        }
    }
}

#[test]
fn hostile_signed_images_are_answered_or_refused_in_bounded_work() {
    let base = sample();
    let mut map = Map::of(&mut base.clone());
    let addrs: Vec<u64> = map.blocks.keys().copied().collect();
    assert!(addrs.len() > 40, "{} blocks", addrs.len());
    let budget = 8 * BLOCKS;
    let cases = if cfg!(debug_assertions) { 150 } else { 1_500 };
    let mut rng = XorShift::new(41);
    let mut most = 0;
    for case in 0..cases {
        let mut disk = base.clone();
        let addr = addrs[rng.below(addrs.len())];
        let (_, ptr, what) = map.blocks[&addr];
        let changed: Block = match (what, rng.below(2)) {
            (What::Node, 0) => {
                changed_node(&mut rng, Node::decode(&block(&disk, addr)).unwrap()).encode()
            }
            _ => {
                let at = ptr.addr + rng.below(ptr.blocks as usize) as u64;
                let mut bytes = hostile::mutate(&mut rng, &block(&disk, at));
                bytes.resize(BLOCK, 0);
                disk.put(at, Rc::new(bytes.try_into().unwrap()));
                block(&disk, addr)
            }
        };
        disk.put(addr, Rc::new(changed));
        map.sign(&mut disk, addr);
        let reads = battery(disk);
        assert!(
            reads < budget,
            "case {case}: {reads} nodes read, block {addr} ({what:?})"
        );
        most = most.max(reads);
        map = Map::of(&mut base.clone());
    }
    eprintln!("{cases} hostile images: at most {most} nodes read");
}

/// Nodes written to unused blocks, `levels` branches high, each of whose
/// `fanout` entries names the one node below; the leaf holds `items`.
fn shared_tree(
    disk: &mut MemDisk,
    at: u64,
    levels: u8,
    fanout: u64,
    items: Vec<(Key, Vec<u8>)>,
) -> BlockPtr {
    let sign = |disk: &mut MemDisk, addr: u64, node: &Node| {
        let bytes = node.encode();
        disk.put(addr, Rc::new(bytes));
        BlockPtr {
            addr,
            birth: 1,
            blocks: 1,
            kind: KIND_NODE,
            level: node.level(),
            flags: 0,
            csum: CsumKind::Seahash64.sum(&bytes),
        }
    };
    let mut ptr = sign(disk, at, &Node::Leaf(items));
    for level in 1..=levels {
        let children = (0..fanout)
            .map(|i| {
                let key = match i {
                    0 => Key::MIN,
                    _ => Key::new(u64::MAX - fanout + i, 0, 0),
                };
                (key, Child::Clean(ptr))
            })
            .collect();
        ptr = sign(disk, at + u64::from(level), &Node::Branch(level, children));
    }
    ptr
}

#[test]
fn a_tree_that_names_one_node_under_every_entry_is_refused_at_its_second_visit() {
    let base = sample();
    let map = Map::of(&mut base.clone());
    // Free blocks for the crafted nodes.
    let free = Fs::mount(base.clone()).unwrap().io.alloc.state().free;
    let &(at, _) = free.iter().find(|(_, len)| *len >= 32).unwrap();
    assert!(map.blocks.range(at..at + 32).next().is_none());
    // A volume's live tree, 12 levels of 40 entries over one leaf: 40^12 leaf
    // visits for a scan that trusts it.
    let mut disk = base.clone();
    let mut fs = Fs::mount(base.clone()).unwrap();
    fs.set_volume(HOME).unwrap();
    let live = fs.live_root().unwrap();
    let mut items: Vec<(Key, Vec<u8>)> = Vec::new();
    fs.scan_raw(&Key::MIN, &mut |k, v| {
        items.push((*k, v.to_vec()));
        items.len() < 20
    })
    .unwrap();
    let root = shared_tree(&mut disk, at, 12, 40, items);
    let mut map = map;
    let (holder, ..) = map.blocks[&live.addr];
    map.point(&mut disk, holder, root);
    let mut fs = Fs::mount(disk).unwrap();
    fs.set_volume(HOME).unwrap();
    assert!(fs.stat(ROOT).is_ok(), "a lookup follows one path");
    // A listing stops at the folder's last name, in the first copy; a walk of
    // the whole tree comes to the second copy and stops there.
    assert!(fs.list(ROOT, None, usize::MAX).is_ok());
    assert!(fs.scan_raw(&Key::MIN, &mut |_, _| true).is_err());
    assert!(fs.changes(0).is_err());
    assert!(fs.io.node_reads < 100, "{} nodes read", fs.io.node_reads);
    let mut cache = Cache::default();
    assert!(check(&mut fs, &mut cache).is_err());
    assert!(fs.io.node_reads < 200, "{} nodes read", fs.io.node_reads);
    // And the volume table itself: the mount refuses it.
    let mut disk = base.clone();
    let mut map = Map::of(&mut disk);
    let mut entries: Vec<(Key, Vec<u8>)> = Vec::new();
    let table = map.sb.volumes;
    if let Node::Leaf(items) = Node::decode(&block(&disk, table.addr)).unwrap() {
        entries = items;
    }
    if !entries.is_empty() {
        let root = shared_tree(&mut disk, at + 16, 12, 40, entries);
        map.point(&mut disk, Holder::Volumes, root);
        assert!(Fs::mount(disk).is_err());
    }
}
