//! Where every block of a good image's newest commit is pointed to from, so a
//! hostile test can change a block and sign it again: the checksum in its
//! pointer, and in every parent's up to the superblock, is rewritten, and the
//! change reaches the parsers behind the checksums. Shared by
//! `tests/hostile_images.rs` and the `kaifs_image` fuzz body
//! (`tests/hostile/image.rs`). Signing never panics on a parent the change
//! left unreadable: it stops there, and the readers find a checksum that does
//! not hold.

#![allow(dead_code)]

use kaifs::format::{self, BlockPtr, PTR_BYTES, Superblock};
use kaifs::fs::record_of;
use kaifs::tree::{Child, Node};
use kaifs::vol::encode_volume;
use kaifs::vol::{K_SNAPSHOT, K_VOLUME, decode_snapshot, decode_volume, encode_snapshot};
use kaifs::{BLOCK, Block, BlockDev, CsumKind, MemDisk};
use std::collections::BTreeMap;
use std::rc::Rc;

/// Where the pointer to a block lies.
#[derive(Clone, Copy, Debug)]
pub enum Holder {
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
pub enum What {
    Node,
    Alloc,
    Record,
}

/// Every block the newest commit reaches, with the first place found that
/// points to it.
#[derive(Clone)]
pub struct Map {
    pub sb: Superblock,
    pub blocks: BTreeMap<u64, (Holder, BlockPtr, What)>,
}

/// Block `addr` of `disk`; one never written reads as zeros.
pub fn block(disk: &MemDisk, addr: u64) -> Block {
    disk.get(addr).map_or([0; BLOCK], |b| **b)
}

impl Map {
    /// The map of a good image; panics on one that is not.
    pub fn of(disk: &mut MemDisk) -> Map {
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
    /// the superblock. An address the map does not hold is left as it is.
    pub fn sign(&mut self, disk: &mut MemDisk, addr: u64) {
        let Some(&(holder, ptr, _)) = self.blocks.get(&addr) else {
            return;
        };
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

    /// Makes `holder` name `ptr`, and signs its block again. A holder that no
    /// longer decodes, or no longer has the item, is left as it is.
    pub fn point(&mut self, disk: &mut MemDisk, holder: Holder, ptr: BlockPtr) {
        let edit = |disk: &mut MemDisk, at: u64, f: &mut dyn FnMut(&mut Node) -> bool| {
            let Ok(mut node) = Node::decode(&block(disk, at)) else {
                return false;
            };
            let done = f(&mut node);
            if done {
                disk.put(at, Rc::new(node.encode()));
            }
            done
        };
        let (up, done) = match holder {
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
                let done = edit(disk, node, &mut |n| match n {
                    Node::Branch(_, children) if index < children.len() => {
                        children[index].1 = Child::Clean(ptr);
                        true
                    }
                    _ => false,
                });
                (node, done)
            }
            Holder::Entry { node, index } => {
                let done = edit(disk, node, &mut |n| {
                    let Node::Leaf(items) = n else { return false };
                    let Some((key, raw)) = items.get_mut(index) else {
                        return false;
                    };
                    let new = match key.kind {
                        K_VOLUME => decode_volume(key.oid, raw).ok().map(|mut entry| {
                            entry.live.root = ptr;
                            encode_volume(&entry)
                        }),
                        _ => decode_snapshot(raw).ok().map(|mut snap| {
                            snap.root = ptr;
                            encode_snapshot(&snap)
                        }),
                    };
                    new.map(|new| *raw = new).is_some()
                });
                (node, done)
            }
            Holder::Record { node, index } => {
                let done = edit(disk, node, &mut |n| match n {
                    Node::Leaf(items)
                        if items.get(index).is_some_and(|i| i.1.len() > PTR_BYTES) =>
                    {
                        items[index].1[1..1 + PTR_BYTES].copy_from_slice(&ptr.encode());
                        true
                    }
                    _ => false,
                });
                (node, done)
            }
            Holder::Prev { block: at } => {
                let mut bytes = block(disk, at);
                bytes[16..16 + PTR_BYTES].copy_from_slice(&ptr.encode());
                disk.put(at, Rc::new(bytes));
                (at, true)
            }
        };
        if done {
            self.sign(disk, up);
        }
    }
}
