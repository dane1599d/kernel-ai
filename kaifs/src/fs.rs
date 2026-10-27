//! The engine (ADR 0004 §1-4): a volume's live tree and its snapshots, the volume
//! table, data records, the allocator, commit and mount.
//!
//! - **Values.** A tree value is inline bytes (up to [`MAX_INLINE`]) or a pointer
//!   to a data record of up to [`MAX_RECORD`] bytes, written copy-on-write like
//!   the nodes. In a leaf a value is a tag byte, then the bytes or the record's
//!   `BlockPtr` and length.
//! - **The volume table** is a second tree of meta data: `(volume, VOLUME, 0)`
//!   holds the volume's live root and `(volume, SNAPSHOT, id)` snapshot `id`'s,
//!   each with the generation it was made in and the volume's next object id.
//!   Object ids are never reused, even across a restore, so the counter lives
//!   here, outside the trees a restore swaps. Task 4 has one volume; task 6 adds
//!   `system`, `home` and `models` and the snapshot kinds.
//! - **Commit G.** Write the dirty nodes and the volume table to free blocks, then
//!   the allocator's part of its chain; FLUSH; write slot G mod 8 in both rings;
//!   FLUSH. There is no journal and no replay.
//! - **Reuse** (§3, as #88 words it). A volume block replaced in G is freed, and
//!   reusable from G+2, only if it was born after the newest snapshot that existed
//!   *when it was replaced* and is not SHARED; any other waits for the GC (task
//!   6). The decision is made at the replacement, never at commit: spike S04-1
//!   found that judging at commit frees blocks a restored snapshot made live
//!   again. Volume-table and allocator blocks belong to no snapshot and are freed.
//! - **Mount** takes the newest superblock whose volume-table root and allocator
//!   head verify; once they do, any later failure is reported, never skipped.

use crate::allocator::{Allocator, run_of};
use crate::buddy::level_for;
use crate::csum::CsumKind;
use crate::dev::BlockDev;
use crate::format::{self, BlockPtr, FLAG_SHARED, KIND_NODE, KIND_RECORD, PTR_BYTES, Superblock};
use crate::tree::{Key, MAX_VALUE, Node, Store, Tree};
use crate::{BLOCK, Block, Error};
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

/// The largest data record: 128 KiB (ADR 0004 §1; sealed 2 MiB blobs come with
/// task 10's model store).
pub const MAX_RECORD: usize = 128 << 10;
/// The largest inline value: a tree value less its tag byte.
pub const MAX_INLINE: usize = MAX_VALUE - 1;

const TAG_INLINE: u8 = 0;
const TAG_RECORD: u8 = 1;
const RECORD_VALUE: usize = 1 + PTR_BYTES + 4;

/// The one volume task 4 has.
pub const VOLUME: u64 = 1;
/// Volume-table item kinds.
pub const K_VOLUME: u8 = 1;
pub const K_SNAPSHOT: u8 = 2;
const ENTRY_FMT: u8 = 1;
const ENTRY_VALUE: usize = 1 + PTR_BYTES + 8 + 8;
/// The first object id a volume hands out; 0 is never one.
pub const FIRST_OID: u64 = 1;

/// A value as a caller sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Inline(Vec<u8>),
    Record(Vec<u8>),
}

impl Value {
    pub fn bytes(&self) -> &[u8] {
        match self {
            Value::Inline(bytes) | Value::Record(bytes) => bytes,
        }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            Value::Inline(bytes) | Value::Record(bytes) => bytes,
        }
    }
}

/// Deliberate bugs, so the crash gates can show they catch each one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Faults {
    /// No flush between a commit's blocks and its superblock.
    pub skip_data_flush: bool,
    /// A replaced block freed at once, reusable in its own transaction.
    pub reuse_now: bool,
    /// Replaced blocks freed whatever snapshots hold them.
    pub ignore_snapshots: bool,
    /// The reuse rule judged at commit rather than at replacement: S04-1's bug.
    pub decide_at_commit: bool,
    /// Freed blocks reusable at N+1: safe with two flushes a commit.
    pub reuse_next: bool,
}

/// A snapshot: its root, null for an empty tree, the generation it was taken in,
/// and the volume's next object id then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snap {
    pub root: BlockPtr,
    pub generation: u64,
    pub next_oid: u64,
}

/// Everything below the trees: the disk, checksums, the allocator and the rule
/// for replaced blocks.
pub struct Io<D> {
    pub dev: D,
    pub kind: CsumKind,
    pub alloc: Allocator,
    /// The generation being built: the last commit's plus one.
    pub txn: u64,
    /// The newest snapshot's generation, 0 with none.
    newest_snap: u64,
    faults: Faults,
    /// Replaced blocks the faulty rule judges at commit.
    deferred: Vec<BlockPtr>,
}

impl<D: BlockDev> Io<D> {
    pub fn load_node(&mut self, ptr: &BlockPtr) -> Result<Node, Error> {
        if ptr.kind != KIND_NODE {
            return Err(Error::Corrupt("node pointer kind"));
        }
        let block = format::read_checked(&mut self.dev, self.kind, ptr, "node")?;
        let node = Node::decode(&block)?;
        match node.level() == ptr.level {
            true => Ok(node),
            false => Err(Error::Corrupt("node level")),
        }
    }

    fn store_node(&mut self, block: &Block, level: u8) -> Result<BlockPtr, Error> {
        let run = self.alloc.alloc(0)?;
        self.dev.write(run.addr, block)?;
        Ok(BlockPtr {
            addr: run.addr,
            birth: self.txn,
            blocks: 1,
            kind: KIND_NODE,
            level,
            flags: 0,
            csum: self.kind.sum(block),
        })
    }

    fn reusable(&self, ptr: &BlockPtr) -> bool {
        ptr.flags & FLAG_SHARED == 0
            && (ptr.birth > self.newest_snap || self.faults.ignore_snapshots)
    }

    /// A volume block replaced in this transaction: freed if no snapshot can hold
    /// it now; else it waits for the GC.
    fn replaced_volume_block(&mut self, ptr: BlockPtr) {
        if self.faults.reuse_now {
            self.alloc.free_now(run_of(&ptr));
        } else if self.faults.decide_at_commit {
            self.deferred.push(ptr);
        } else if self.reusable(&ptr) {
            self.alloc.free_later(self.txn, run_of(&ptr));
        }
    }

    /// Writes `data` as a record to a free run and returns its pointer.
    fn write_record(&mut self, data: &[u8]) -> Result<BlockPtr, Error> {
        let blocks = data.len().div_ceil(BLOCK);
        let run = self.alloc.alloc(level_for(blocks as u64))?;
        let mut padded = vec![0; blocks * BLOCK];
        padded[..data.len()].copy_from_slice(data);
        for (i, chunk) in padded.as_chunks::<BLOCK>().0.iter().enumerate() {
            self.dev.write(run.addr + i as u64, chunk)?;
        }
        Ok(BlockPtr {
            addr: run.addr,
            birth: self.txn,
            blocks: blocks as u32,
            kind: KIND_RECORD,
            level: 0,
            flags: 0,
            csum: self.kind.sum(&padded),
        })
    }

    /// Reads and verifies the `len` bytes of the record `ptr` names.
    pub fn read_record(&mut self, ptr: &BlockPtr, len: usize) -> Result<Vec<u8>, Error> {
        let bad = Error::Corrupt("record");
        let (start, end) = format::data_area(self.dev.info().blocks);
        let blocks = ptr.blocks as usize;
        if ptr.kind != KIND_RECORD
            || blocks == 0
            || blocks * BLOCK > MAX_RECORD
            || len > blocks * BLOCK
            || len <= (blocks - 1) * BLOCK
            || ptr.addr < start
            || ptr.addr + blocks as u64 > end
        {
            return Err(bad);
        }
        let mut data = vec![0; blocks * BLOCK];
        for (i, chunk) in data.as_chunks_mut::<BLOCK>().0.iter_mut().enumerate() {
            self.dev.read(ptr.addr + i as u64, chunk)?;
        }
        if self.kind.sum(&data) != ptr.csum {
            return Err(bad);
        }
        data.truncate(len);
        Ok(data)
    }
}

/// The live tree's and the snapshots' view of the disk: replaced blocks follow the
/// reuse rule.
struct Volume<'a, D>(&'a mut Io<D>);

/// The volume table's: its replaced blocks are always freed.
struct Meta<'a, D>(&'a mut Io<D>);

impl<D: BlockDev> Store for Volume<'_, D> {
    fn load(&mut self, ptr: &BlockPtr) -> Result<Node, Error> {
        self.0.load_node(ptr)
    }

    fn replaced(&mut self, ptr: BlockPtr) {
        self.0.replaced_volume_block(ptr);
    }

    fn store(&mut self, block: &Block, level: u8) -> Result<BlockPtr, Error> {
        self.0.store_node(block, level)
    }
}

impl<D: BlockDev> Store for Meta<'_, D> {
    fn load(&mut self, ptr: &BlockPtr) -> Result<Node, Error> {
        self.0.load_node(ptr)
    }

    fn replaced(&mut self, ptr: BlockPtr) {
        let txn = self.0.txn;
        self.0.alloc.free_later(txn, run_of(&ptr));
    }

    fn store(&mut self, block: &Block, level: u8) -> Result<BlockPtr, Error> {
        self.0.store_node(block, level)
    }
}

fn encode_entry(snap: &Snap) -> Vec<u8> {
    let mut value = Vec::with_capacity(ENTRY_VALUE);
    value.push(ENTRY_FMT);
    value.extend_from_slice(&snap.root.encode());
    value.extend_from_slice(&snap.generation.to_le_bytes());
    value.extend_from_slice(&snap.next_oid.to_le_bytes());
    value
}

/// A volume-table entry. Longer entries are a later minor version's: their known
/// prefix is read.
fn decode_entry(value: &[u8]) -> Result<Snap, Error> {
    if value.len() < ENTRY_VALUE || value[0] != ENTRY_FMT {
        return Err(Error::Corrupt("volume table entry"));
    }
    let root = BlockPtr::decode_at(value, 1)?;
    if !root.is_null() && root.kind != KIND_NODE {
        return Err(Error::Corrupt("volume root kind"));
    }
    let word = |at: usize| {
        let mut word = [0; 8];
        word.copy_from_slice(&value[at..at + 8]);
        u64::from_le_bytes(word)
    };
    let next_oid = word(1 + PTR_BYTES + 8);
    if next_oid < FIRST_OID {
        return Err(Error::Corrupt("volume table entry"));
    }
    Ok(Snap {
        root,
        generation: word(1 + PTR_BYTES),
        next_oid,
    })
}

/// A raw tree value as the caller's value, reading a record through `io`.
pub fn decode_value<D: BlockDev>(raw: &[u8], io: &mut Io<D>) -> Result<Value, Error> {
    match raw.first() {
        Some(&TAG_INLINE) => Ok(Value::Inline(raw[1..].to_vec())),
        Some(&TAG_RECORD) => {
            let (ptr, len) = record_of(raw).ok_or(Error::Corrupt("record value"))?;
            io.read_record(&ptr, len).map(Value::Record)
        }
        _ => Err(Error::Corrupt("value tag")),
    }
}

/// The raw value holding `bytes` inline.
pub(crate) fn inline_raw(bytes: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(1 + bytes.len());
    raw.push(TAG_INLINE);
    raw.extend_from_slice(bytes);
    raw
}

/// A raw value's inline bytes, if it holds them inline.
pub fn inline_of(raw: &[u8]) -> Option<&[u8]> {
    match raw.first() {
        Some(&TAG_INLINE) => Some(&raw[1..]),
        _ => None,
    }
}

/// The bytes a raw value holds, inline or in its record, without reading the
/// record.
pub fn value_len(raw: &[u8]) -> Option<usize> {
    match raw.first() {
        Some(&TAG_INLINE) => Some(raw.len() - 1),
        _ => record_of(raw).map(|(_, len)| len),
    }
}

/// The record a raw value names, if it names one: its pointer and length.
pub fn record_of(raw: &[u8]) -> Option<(BlockPtr, usize)> {
    if raw.first() != Some(&TAG_RECORD) || raw.len() != RECORD_VALUE {
        return None;
    }
    let ptr = BlockPtr::decode_at(raw, 1).ok()?;
    let mut word = [0; 4];
    word.copy_from_slice(&raw[1 + PTR_BYTES..]);
    Some((ptr, u32::from_le_bytes(word) as usize))
}

fn live_key() -> Key {
    Key::new(VOLUME, K_VOLUME, 0)
}

fn snap_key(id: u64) -> Key {
    Key::new(VOLUME, K_SNAPSHOT, id)
}

pub struct Fs<D> {
    pub io: Io<D>,
    pub(crate) uuid: [u8; 16],
    block_count: u64,
    label: [u8; 64],
    /// The last commit's generation.
    pub committed: u64,
    /// The writer's clock for the next superblock, nanoseconds since 1970.
    pub clock: u64,
    live: Tree,
    live_changed: bool,
    volumes: Tree,
    snaps: BTreeMap<u64, Snap>,
    next_snap: u64,
    /// The volume's next object id.
    next_oid: u64,
    writable: bool,
    /// No unknown compat bit is set, so `check --repair` may write.
    repairable: bool,
    pub faults: Faults,
    /// The bits of a name's hash the namespace keeps: all of them on disk; tests
    /// narrow it to make names collide.
    pub hash_mask: u64,
}

impl<D: BlockDev> Fs<D> {
    /// Writes an empty filesystem over all of `dev`, as generation 1.
    pub fn format(dev: D, uuid: [u8; 16], label: &str) -> Result<Self, Error> {
        Self::format_with(dev, uuid, label, Faults::default())
    }

    /// [`Fs::format`] with deliberate faults, for the crash gates.
    pub fn format_with(dev: D, uuid: [u8; 16], label: &str, faults: Faults) -> Result<Self, Error> {
        let mut fs = Self::build(dev, uuid, label, faults)?;
        fs.commit()?;
        Ok(fs)
    }

    /// An empty filesystem over all of `dev`, not yet committed.
    pub(crate) fn build(
        mut dev: D,
        uuid: [u8; 16],
        label: &str,
        faults: Faults,
    ) -> Result<Self, Error> {
        let info = dev.info();
        if info.blocks < format::MIN_BLOCKS {
            return Err(Error::NoSpace);
        }
        if !info.durable() {
            return Err(Error::Unsupported("a write-back cache without flush"));
        }
        format::clear_rings(&mut dev, info.blocks)?;
        let mut alloc = Allocator::new(format::data_area(info.blocks));
        alloc.faults.reuse_next = faults.reuse_next;
        let mut name = [0; 64];
        let n = label.len().min(64);
        name[..n].copy_from_slice(&label.as_bytes()[..n]);
        let fs = Fs {
            io: Io {
                dev,
                kind: CsumKind::Seahash64,
                alloc,
                txn: 1,
                newest_snap: 0,
                faults,
                deferred: Vec::new(),
            },
            uuid,
            block_count: info.blocks,
            label: name,
            committed: 0,
            clock: 0,
            live: Tree::default(),
            live_changed: true,
            volumes: Tree::default(),
            snaps: BTreeMap::new(),
            next_snap: 1,
            next_oid: FIRST_OID,
            writable: true,
            repairable: true,
            faults,
            hash_mask: u64::MAX,
        };
        Ok(fs)
    }

    /// Mounts the newest commit whose roots verify.
    pub fn mount(dev: D) -> Result<Self, Error> {
        Self::mount_with(dev, Faults::default())
    }

    /// [`Fs::mount`] for a writer with deliberate faults (its reuse wait too).
    pub fn mount_with(mut dev: D, faults: Faults) -> Result<Self, Error> {
        let info = dev.info();
        let found = format::read_rings(&mut dev)?;
        let rules = found[0].rules()?;
        let kind = found[0].csum_kind;
        for sb in found {
            let verifies = |dev: &mut D, ptr: &BlockPtr| {
                ptr.is_null() || format::read_checked(dev, kind, ptr, "root").is_ok()
            };
            if sb.volumes.is_null()
                || !verifies(&mut dev, &sb.volumes)
                || !verifies(&mut dev, &sb.alloc)
            {
                continue;
            }
            let mut fs = Self::open(dev, &sb, faults)?;
            fs.writable = rules.writable && info.durable();
            fs.repairable = rules.repairable;
            return Ok(fs);
        }
        Err(Error::NoValidSuperblock)
    }

    fn open(mut dev: D, sb: &Superblock, faults: Faults) -> Result<Self, Error> {
        let alloc_faults = crate::allocator::Faults {
            reuse_next: faults.reuse_next,
            ..Default::default()
        };
        let alloc = Allocator::load_with(
            &mut dev,
            sb.csum_kind,
            &sb.alloc,
            sb.generation,
            alloc_faults,
        )?;
        let mut io = Io {
            dev,
            kind: sb.csum_kind,
            alloc,
            txn: sb.generation + 1,
            newest_snap: 0,
            faults,
            deferred: Vec::new(),
        };
        let volumes = Tree::open(sb.volumes);
        let mut entries: Vec<(Key, Vec<u8>)> = Vec::new();
        volumes.scan(&Key::MIN, &mut Meta(&mut io), &mut |k, v| {
            entries.push((*k, v.to_vec()));
            true
        })?;
        let mut live = None;
        let mut snaps = BTreeMap::new();
        for (key, value) in entries {
            let entry = decode_entry(&value)?;
            match (key.oid, key.kind) {
                (VOLUME, K_VOLUME) if key.off == 0 => live = Some(entry),
                (VOLUME, K_SNAPSHOT) if key.off > 0 => {
                    snaps.insert(key.off, entry);
                }
                _ => return Err(Error::Corrupt("an unknown volume table item")),
            }
        }
        let live = live.ok_or(Error::Corrupt("no live root"))?;
        io.newest_snap = snaps.values().map(|s| s.generation).max().unwrap_or(0);
        let next_snap = snaps.keys().next_back().map_or(1, |id| id + 1);
        Ok(Fs {
            io,
            uuid: sb.uuid,
            block_count: sb.block_count,
            label: sb.label,
            committed: sb.generation,
            clock: sb.commit_time,
            live: Tree::open(live.root),
            live_changed: false,
            volumes,
            snaps,
            next_snap,
            next_oid: live.next_oid,
            writable: true,
            repairable: true,
            faults,
            hash_mask: u64::MAX,
        })
    }

    pub fn into_dev(self) -> D {
        self.io.dev
    }

    pub fn writable(&self) -> bool {
        self.writable
    }

    pub fn block_count(&self) -> u64 {
        self.block_count
    }

    pub fn uuid(&self) -> [u8; 16] {
        self.uuid
    }

    /// Whether `check --repair` may write: no unknown compat bit is set.
    pub fn repairable(&self) -> bool {
        self.repairable
    }

    /// The volume's next object id.
    pub fn next_oid(&self) -> u64 {
        self.next_oid
    }

    /// Hands out a new object id; none is ever handed out twice.
    pub fn alloc_oid(&mut self) -> Result<u64, Error> {
        let oid = self.next_oid;
        self.next_oid = oid.checked_add(1).ok_or(Error::NoSpace)?;
        self.live_changed = true;
        Ok(oid)
    }

    /// Raises the counter past `oid`: `check --repair` found an object above it.
    pub(crate) fn bump_oid(&mut self, oid: u64) {
        if oid >= self.next_oid {
            self.next_oid = oid.saturating_add(1);
            self.live_changed = true;
        }
    }

    /// The raw tree value at `key`: records are named, not read.
    pub fn get_raw(&mut self, key: &Key) -> Result<Option<Vec<u8>>, Error> {
        self.live.get(key, &mut Volume(&mut self.io))
    }

    pub fn get(&mut self, key: &Key) -> Result<Option<Value>, Error> {
        match self.live.get(key, &mut Volume(&mut self.io))? {
            Some(raw) => decode_value(&raw, &mut self.io).map(Some),
            None => Ok(None),
        }
    }

    /// Sets `key` to inline bytes, at most [`MAX_INLINE`] of them.
    pub fn put(&mut self, key: Key, value: &[u8]) -> Result<(), Error> {
        if value.len() > MAX_INLINE {
            return Err(Error::Invalid("an inline value over the limit"));
        }
        let mut raw = Vec::with_capacity(1 + value.len());
        raw.push(TAG_INLINE);
        raw.extend_from_slice(value);
        self.set(key, raw)
    }

    /// Sets `key` to a record of 1 to [`MAX_RECORD`] bytes, written copy-on-write.
    pub fn put_record(&mut self, key: Key, data: &[u8]) -> Result<(), Error> {
        if data.is_empty() || data.len() > MAX_RECORD {
            return Err(Error::Invalid("a record of no bytes or over 128 KiB"));
        }
        let ptr = self.io.write_record(data)?;
        let mut raw = Vec::with_capacity(RECORD_VALUE);
        raw.push(TAG_RECORD);
        raw.extend_from_slice(&ptr.encode());
        raw.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.set(key, raw)
    }

    /// Sets `key` to a raw value as `get_raw` returned it; `check --repair` moves
    /// items this way.
    pub(crate) fn set(&mut self, key: Key, raw: Vec<u8>) -> Result<(), Error> {
        let old = self.live.insert(key, raw, &mut Volume(&mut self.io))?;
        self.release_value(old);
        self.live_changed = true;
        Ok(())
    }

    /// Removes `key`; returns whether it was there.
    pub fn delete(&mut self, key: &Key) -> Result<bool, Error> {
        let old = self.live.remove(key, &mut Volume(&mut self.io))?;
        let found = old.is_some();
        self.release_value(old);
        self.live_changed |= found;
        Ok(found)
    }

    fn release_value(&mut self, old: Option<Vec<u8>>) {
        if let Some((ptr, _)) = old.as_deref().and_then(record_of) {
            self.io.replaced_volume_block(ptr);
        }
    }

    /// Calls `visit` on every raw item from `from` on, in key order, until it
    /// returns false; records are named, not read.
    pub fn scan_raw(
        &mut self,
        from: &Key,
        visit: &mut dyn FnMut(&Key, &[u8]) -> bool,
    ) -> Result<(), Error> {
        self.live.scan(from, &mut Volume(&mut self.io), visit)
    }

    /// Calls `visit` on every raw item of the tree `root` names (a snapshot's, or
    /// the live tree's as last committed) from `from` on, until it returns false.
    pub fn scan_tree(
        &mut self,
        root: &BlockPtr,
        from: &Key,
        visit: &mut dyn FnMut(&Key, &[u8]) -> bool,
    ) -> Result<(), Error> {
        Tree::open(*root).scan(from, &mut Volume(&mut self.io), visit)
    }

    /// Makes `items`, raw and in key order, the whole live tree. The old tree's
    /// blocks are never read again and wait for the GC: `check --repair` rebuilds
    /// a tree it could not wholly read this way.
    pub(crate) fn replace_live(&mut self, items: Vec<(Key, Vec<u8>)>) -> Result<(), Error> {
        let mut tree = Tree::default();
        for (key, raw) in items {
            tree.insert(key, raw, &mut Volume(&mut self.io))?;
        }
        self.live = tree;
        self.live_changed = true;
        Ok(())
    }

    /// Calls `visit` on every item from `from` on, in key order, until it returns
    /// false; records are read and verified.
    pub fn scan(
        &mut self,
        from: &Key,
        visit: &mut dyn FnMut(&Key, Value) -> bool,
    ) -> Result<(), Error> {
        // In batches, since a record is read through the same `io` the scan uses.
        const BATCH: usize = 256;
        let mut raws: Vec<(Key, Vec<u8>)> = Vec::new();
        let mut cursor = Some(*from);
        while let Some(at) = cursor {
            raws.clear();
            self.live
                .scan(&at, &mut Volume(&mut self.io), &mut |k, v| {
                    raws.push((*k, v.to_vec()));
                    raws.len() < BATCH
                })?;
            cursor = match raws.len() == BATCH {
                true => raws.last().and_then(|(k, _)| k.next()),
                false => None,
            };
            for (key, raw) in raws.drain(..) {
                if !visit(&key, decode_value(&raw, &mut self.io)?) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Takes a snapshot of the live tree as it is now: its dirty nodes are written
    /// (no flush), and its root joins the volume table at the next commit.
    pub fn snapshot(&mut self) -> Result<u64, Error> {
        let root = self.live.write_out(&mut Volume(&mut self.io))?;
        let id = self.next_snap;
        self.next_snap += 1;
        let snap = Snap {
            root,
            generation: self.io.txn,
            next_oid: self.next_oid,
        };
        self.snaps.insert(id, snap);
        self.io.newest_snap = self.io.txn;
        self.volumes
            .insert(snap_key(id), encode_entry(&snap), &mut Meta(&mut self.io))?;
        Ok(id)
    }

    pub fn drop_snapshot(&mut self, id: u64) -> Result<bool, Error> {
        if self.snaps.remove(&id).is_none() {
            return Ok(false);
        }
        self.io.newest_snap = self.snaps.values().map(|s| s.generation).max().unwrap_or(0);
        self.volumes
            .remove(&snap_key(id), &mut Meta(&mut self.io))?;
        Ok(true)
    }

    /// Makes snapshot `id`'s tree the live tree. The live tree's own blocks wait
    /// for the GC; the object id counter never goes back.
    pub fn restore(&mut self, id: u64) -> Result<bool, Error> {
        let Some(snap) = self.snaps.get(&id) else {
            return Ok(false);
        };
        self.live = Tree::open(snap.root);
        self.live_changed = true;
        Ok(true)
    }

    pub fn snapshots(&self) -> &BTreeMap<u64, Snap> {
        &self.snaps
    }

    /// Commits the transaction: two flushes, both rings. Returns the generation.
    pub fn commit(&mut self) -> Result<u64, Error> {
        if !self.writable {
            return Err(Error::ReadOnly);
        }
        let generation = self.io.txn;
        if self.live_changed {
            let root = self.live.write_out(&mut Volume(&mut self.io))?;
            let live = Snap {
                root,
                generation,
                next_oid: self.next_oid,
            };
            self.volumes
                .insert(live_key(), encode_entry(&live), &mut Meta(&mut self.io))?;
            self.live_changed = false;
        }
        let volumes = self.volumes.write_out(&mut Meta(&mut self.io))?;
        for ptr in core::mem::take(&mut self.io.deferred) {
            if self.io.reusable(&ptr) {
                self.io.alloc.free_later(generation, run_of(&ptr));
            }
        }
        let head = self
            .io
            .alloc
            .persist(&mut self.io.dev, self.io.kind, generation)?;
        if !self.faults.skip_data_flush {
            self.io.dev.flush()?;
        }
        let mut sb = Superblock::new(self.uuid, self.block_count, generation);
        sb.csum_kind = self.io.kind;
        sb.volumes = volumes;
        sb.alloc = head;
        sb.commit_time = self.clock;
        sb.label = self.label;
        format::write_rings(&mut self.io.dev, &sb)?;
        self.io.dev.flush()?;
        self.committed = generation;
        self.io.txn = generation + 1;
        self.io.alloc.committed(generation);
        Ok(generation)
    }

    /// The roots the last commit wrote, for `check()`: the live tree's (null when
    /// empty, or when it has changed since), the volume table's, and the
    /// allocator's chain.
    pub fn live_root(&self) -> Option<BlockPtr> {
        match &self.live.root {
            None => Some(BlockPtr::NULL),
            Some(crate::tree::Child::Clean(ptr)) => Some(*ptr),
            Some(crate::tree::Child::Dirty(_)) => None,
        }
    }

    pub fn volumes_root(&self) -> Option<BlockPtr> {
        match &self.volumes.root {
            Some(crate::tree::Child::Clean(ptr)) => Some(*ptr),
            _ => None,
        }
    }

    /// Whether nothing has changed since the last commit.
    pub fn is_clean(&self) -> bool {
        !self.live_changed && !self.volumes.is_dirty() && self.io.alloc.is_idle()
    }
}
