//! The filesystem: one live tree, its snapshots, the volume table, the allocator,
//! commit, mount, GC and `check()` (ADR 0004 §1-4).
//!
//! - **Values.** A tree value is inline bytes (at most 1 KiB) or a pointer to a
//!   data record of up to 128 KiB, written copy-on-write like the nodes.
//! - **Volume table.** A second tree: key 0 holds the live root, key `id` snapshot
//!   `id`'s root, each with the generation that wrote it.
//! - **Commit G.** Write every dirty node and record and the allocator chain to
//!   free blocks; FLUSH; write slot G mod 8 in both rings; FLUSH.
//! - **Frees.** A block replaced in commit G is reusable from G+2 if it was born
//!   after the newest snapshot; any other waits for the GC.

use crate::alloc_buddy::{Buddy, Run, level_for};
use crate::csum::CsumKind;
use crate::disk::BlockDev;
use crate::format::{self, BlockPtr, KIND_ALLOC, KIND_NODE, KIND_RECORD, Superblock};
use crate::tree::{Child, Node, Store, Tree};
use crate::{BLOCK, Block, MAX_RECORD, MAX_VALUE};
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec;
use alloc::vec::Vec;
use zerocopy::{FromBytes, IntoBytes};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Io,
    /// A checksum or structure failed: the named thing.
    Corrupt(&'static str),
    NoSpace,
    /// No superblock slot holds a commit that verifies.
    NoValidSuperblock,
    /// The format names something this build does not know.
    Unsupported,
}

impl From<crate::disk::IoError> for Error {
    fn from(_: crate::disk::IoError) -> Self {
        Error::Io
    }
}

/// A value as a caller sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Inline(Vec<u8>),
    Record(Vec<u8>),
}

const TAG_INLINE: u8 = 0;
const TAG_RECORD: u8 = 1;
/// A record value: tag, `BlockPtr`, length.
const RECORD_VALUE: usize = 1 + 64 + 4;
/// A volume-table value: `BlockPtr` (null for an empty tree) and generation.
const VOLUME_VALUE: usize = 64 + 8;
const LIVE: u64 = 0;

/// Everything below the trees: the disk, checksums and the allocator.
pub struct Io<D> {
    pub dev: D,
    pub kind: CsumKind,
    pub alloc: Buddy,
    /// The generation being built: the last commit's plus one.
    pub txn: u64,
    /// Blocks replaced in this transaction, and whether each may be reused at
    /// G+2: decided when it is replaced, since a later restore can make a block
    /// that a snapshot held live again.
    freed: Vec<(BlockPtr, bool)>,
    /// Replacing volume-table blocks, which no snapshot holds.
    meta: bool,
    /// The newest snapshot's generation, 0 with none.
    newest_snap: u64,
    /// [`Faults::ignore_snapshots`].
    ignore_snapshots: bool,
    /// [`Faults::reuse_now`].
    pub reuse_now: bool,
}

fn read_block<D: BlockDev>(dev: &D, addr: u64) -> Result<Box<Block>, Error> {
    let mut block = Box::new([0; BLOCK]);
    dev.read(addr, &mut block)?;
    Ok(block)
}

impl<D: BlockDev> Io<D> {
    /// Reads the block `ptr` names and verifies its checksum.
    fn read_verified(&self, ptr: &BlockPtr, what: &'static str) -> Result<Box<Block>, Error> {
        if ptr.blocks != 1 {
            return Err(Error::Corrupt(what));
        }
        let block = read_block(&self.dev, ptr.addr)?;
        if self.kind.sum(&block[..]) != ptr.csum {
            return Err(Error::Corrupt(what));
        }
        Ok(block)
    }

    fn write_new(&mut self, block: &Block, kind: u8, level: u8) -> Result<BlockPtr, Error> {
        let run = self.alloc.alloc(0).ok_or(Error::NoSpace)?;
        self.dev.write(run.addr, block)?;
        Ok(BlockPtr {
            addr: run.addr,
            birth: self.txn,
            blocks: 1,
            kind,
            level,
            csum: self.kind.sum(block),
            ..BlockPtr::default()
        })
    }

    fn read_record(&self, ptr: &BlockPtr, len: usize) -> Result<Vec<u8>, Error> {
        let bad = Error::Corrupt("record");
        if ptr.kind != KIND_RECORD
            || ptr.blocks == 0
            || ptr.blocks as usize * BLOCK > MAX_RECORD
            || len > ptr.blocks as usize * BLOCK
        {
            return Err(bad);
        }
        let mut data = vec![0; ptr.blocks as usize * BLOCK];
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

impl<D: BlockDev> Store for Io<D> {
    fn load(&mut self, ptr: &BlockPtr) -> Result<Node, Error> {
        if ptr.kind != KIND_NODE {
            return Err(Error::Corrupt("node pointer kind"));
        }
        let node = Node::decode(&*self.read_verified(ptr, "node")?)?;
        if node.level() != ptr.level {
            return Err(Error::Corrupt("node level"));
        }
        Ok(node)
    }

    fn freed(&mut self, ptr: BlockPtr) {
        if self.reuse_now {
            self.alloc.free_now(run_of(&ptr));
            return;
        }
        let reusable = self.meta || ptr.birth > self.newest_snap || self.ignore_snapshots;
        self.freed.push((ptr, reusable));
    }

    fn store(&mut self, block: &Block, level: u8) -> Result<BlockPtr, Error> {
        self.write_new(block, KIND_NODE, level)
    }
}

/// Deliberate bugs, so the crash harness can show it catches each one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Faults {
    /// Commit without the flush between the data and the superblocks.
    pub skip_data_flush: bool,
    /// Reuse a freed block one commit later instead of two. Safe with two flushes
    /// a commit: the harness finds no violation, so N+2 is a margin.
    pub reuse_next: bool,
    /// Reuse a replaced block within the same transaction.
    pub reuse_now: bool,
    /// Free replaced blocks at once, ignoring snapshots.
    pub ignore_snapshots: bool,
}

/// A snapshot: its root (None for an empty tree) and the generation it was taken in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snap {
    pub root: Option<BlockPtr>,
    pub generation: u64,
}

pub struct Fs<D> {
    pub io: Io<D>,
    uuid: [u8; 16],
    block_count: u64,
    /// The last commit's generation.
    pub committed: u64,
    live: Tree,
    live_changed: bool,
    volumes: Tree,
    pub snaps: BTreeMap<u64, Snap>,
    next_snap: u64,
    /// The allocator chain the last commit wrote.
    alloc_chain: Vec<BlockPtr>,
    pub faults: Faults,
}

fn encode_volume(root: Option<BlockPtr>, generation: u64) -> Vec<u8> {
    let mut value = Vec::with_capacity(VOLUME_VALUE);
    value.extend_from_slice(root.unwrap_or_default().as_bytes());
    value.extend_from_slice(&generation.to_le_bytes());
    value
}

fn decode_volume(value: &[u8]) -> Result<Snap, Error> {
    if value.len() != VOLUME_VALUE {
        return Err(Error::Corrupt("volume entry"));
    }
    let (ptr, _) = BlockPtr::read_from_prefix(value).map_err(|_| Error::Corrupt("volume entry"))?;
    let generation = u64::from_le_bytes(value[64..72].try_into().unwrap());
    Ok(Snap {
        root: (!ptr.is_null()).then_some(ptr),
        generation,
    })
}

impl<D: BlockDev> Fs<D> {
    /// Writes an empty filesystem of `dev`'s size as generation 1.
    pub fn format(dev: D, kind: CsumKind, uuid: [u8; 16]) -> Result<Self, Error> {
        let block_count = dev.blocks();
        let (start, end) = format::data_area(block_count);
        let mut fs = Fs {
            io: Io {
                dev,
                kind,
                alloc: Buddy::new(start, end),
                txn: 1,
                freed: Vec::new(),
                meta: false,
                newest_snap: 0,
                ignore_snapshots: false,
                reuse_now: false,
            },
            uuid,
            block_count,
            committed: 0,
            live: Tree::default(),
            live_changed: true,
            volumes: Tree::default(),
            snaps: BTreeMap::new(),
            next_snap: 1,
            alloc_chain: Vec::new(),
            faults: Faults::default(),
        };
        fs.commit()?;
        Ok(fs)
    }

    /// Mounts the newest commit whose superblock, volume table root and allocator
    /// chain verify. No journal, no replay.
    pub fn mount(dev: D) -> Result<Self, Error> {
        let block_count = dev.blocks();
        let mut candidates = Vec::new();
        for addr in format::all_slots(block_count) {
            if let Some(sb) = Superblock::decode(&*read_block(&dev, addr)?) {
                candidates.push(sb);
            }
        }
        candidates.sort_by_key(|sb| core::cmp::Reverse(sb.generation));
        let mut dev = Some(dev);
        for sb in candidates {
            if sb.format_major < format::EARLIEST_SUPPORTED
                || sb.format_major > format::FORMAT_MAJOR
            {
                return Err(Error::Unsupported);
            }
            if sb.incompat & !format::INCOMPAT_KNOWN != 0 {
                return Err(Error::Unsupported);
            }
            if sb.block_count != block_count {
                continue;
            }
            match Self::open(dev.take().unwrap(), &sb) {
                Ok(fs) => return Ok(fs),
                Err((back, _)) => dev = Some(back),
            }
        }
        Err(Error::NoValidSuperblock)
    }

    fn open(dev: D, sb: &Superblock) -> Result<Self, (D, Error)> {
        let kind = match CsumKind::from_u8(sb.csum_kind) {
            Some(kind) => kind,
            None => return Err((dev, Error::Unsupported)),
        };
        let mut io = Io {
            dev,
            kind,
            alloc: Buddy::default(),
            txn: sb.generation + 1,
            freed: Vec::new(),
            meta: false,
            newest_snap: 0,
            ignore_snapshots: false,
            reuse_now: false,
        };
        let loaded = (|| {
            let (alloc, chain) = load_alloc(&io, &sb.alloc)?;
            io.alloc = alloc;
            let volumes = Tree {
                root: Some(Child::Clean(sb.volumes)),
            };
            let snaps = read_volumes(&volumes, &mut io)?;
            Ok((volumes, snaps, chain))
        })();
        let (volumes, mut snaps, chain) = match loaded {
            Ok(parts) => parts,
            Err(error) => return Err((io.dev, error)),
        };
        let live = match snaps.remove(&LIVE) {
            Some(snap) => snap,
            None => return Err((io.dev, Error::Corrupt("no live root"))),
        };
        io.alloc.release(io.txn);
        io.newest_snap = snaps.values().map(|s| s.generation).max().unwrap_or(0);
        let next_snap = snaps.keys().next_back().map_or(1, |id| id + 1);
        Ok(Fs {
            io,
            uuid: sb.uuid,
            block_count: sb.block_count,
            committed: sb.generation,
            live: Tree {
                root: live.root.map(Child::Clean),
            },
            live_changed: false,
            volumes,
            snaps,
            next_snap,
            alloc_chain: chain,
            faults: Faults::default(),
        })
    }

    pub fn into_dev(self) -> D {
        self.io.dev
    }

    pub fn get(&mut self, key: u64) -> Result<Option<Value>, Error> {
        let Some(raw) = self.live.get(key, &mut self.io)? else {
            return Ok(None);
        };
        decode_value(&raw, &self.io).map(Some)
    }

    /// Sets `key` to inline bytes (at most 1 KiB).
    pub fn put(&mut self, key: u64, value: &[u8]) -> Result<(), Error> {
        assert!(value.len() < MAX_VALUE);
        let mut raw = Vec::with_capacity(1 + value.len());
        raw.push(TAG_INLINE);
        raw.extend_from_slice(value);
        self.set(key, raw)
    }

    /// Sets `key` to a data record of up to 128 KiB, written copy-on-write.
    pub fn put_record(&mut self, key: u64, data: &[u8]) -> Result<(), Error> {
        assert!(!data.is_empty() && data.len() <= MAX_RECORD);
        let blocks = data.len().div_ceil(BLOCK);
        let run = self
            .io
            .alloc
            .alloc(level_for(blocks as u64))
            .ok_or(Error::NoSpace)?;
        let mut padded = vec![0; blocks * BLOCK];
        padded[..data.len()].copy_from_slice(data);
        for (i, chunk) in padded.as_chunks::<BLOCK>().0.iter().enumerate() {
            self.io.dev.write(run.addr + i as u64, chunk)?;
        }
        let ptr = BlockPtr {
            addr: run.addr,
            birth: self.io.txn,
            blocks: blocks as u32,
            kind: KIND_RECORD,
            csum: self.io.kind.sum(&padded),
            ..BlockPtr::default()
        };
        let mut raw = Vec::with_capacity(RECORD_VALUE);
        raw.push(TAG_RECORD);
        raw.extend_from_slice(ptr.as_bytes());
        raw.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.set(key, raw)
    }

    fn set(&mut self, key: u64, raw: Vec<u8>) -> Result<(), Error> {
        if let Some(old) = self.live.insert(key, raw, &mut self.io)? {
            self.free_value(&old);
        }
        self.live_changed = true;
        Ok(())
    }

    /// Removes `key`; returns whether it was there.
    pub fn delete(&mut self, key: u64) -> Result<bool, Error> {
        if self.live.get(key, &mut self.io)?.is_none() {
            return Ok(false);
        }
        if let Some(old) = self.live.remove(key, &mut self.io)? {
            self.free_value(&old);
        }
        self.live_changed = true;
        Ok(true)
    }

    fn free_value(&mut self, raw: &[u8]) {
        if raw.first() == Some(&TAG_RECORD)
            && let Ok((ptr, _)) = BlockPtr::read_from_prefix(&raw[1..])
        {
            self.io.freed(ptr);
        }
    }

    /// Takes a snapshot of the live tree as it is now: its dirty nodes are written
    /// (no flush), and its root joins the volume table at the next commit.
    pub fn snapshot(&mut self) -> Result<u64, Error> {
        let root = self.live.write_out(&mut self.io)?;
        let id = self.next_snap;
        self.next_snap += 1;
        let snap = Snap {
            root,
            generation: self.io.txn,
        };
        self.snaps.insert(id, snap);
        self.io.newest_snap = self.newest_snapshot();
        self.io.meta = true;
        let result = self
            .volumes
            .insert(id, encode_volume(root, snap.generation), &mut self.io);
        self.io.meta = false;
        result.map(|_| id)
    }

    pub fn drop_snapshot(&mut self, id: u64) -> Result<bool, Error> {
        if self.snaps.remove(&id).is_none() {
            return Ok(false);
        }
        self.io.newest_snap = self.newest_snapshot();
        self.io.meta = true;
        let result = self.volumes.remove(id, &mut self.io);
        self.io.meta = false;
        result.map(|_| true)
    }

    /// Makes snapshot `id`'s tree the live tree. The live tree's own blocks wait
    /// for the GC.
    pub fn restore(&mut self, id: u64) -> Result<bool, Error> {
        let Some(snap) = self.snaps.get(&id) else {
            return Ok(false);
        };
        self.live = Tree {
            root: snap.root.map(Child::Clean),
        };
        self.live_changed = true;
        Ok(true)
    }

    fn newest_snapshot(&self) -> u64 {
        self.snaps.values().map(|s| s.generation).max().unwrap_or(0)
    }

    /// Commits the transaction: two flushes, both rings. Returns the generation.
    pub fn commit(&mut self) -> Result<u64, Error> {
        let generation = self.io.txn;
        if self.live_changed {
            let root = self.live.write_out(&mut self.io)?;
            self.io.meta = true;
            self.volumes
                .insert(LIVE, encode_volume(root, generation), &mut self.io)?;
            self.io.meta = false;
            self.live_changed = false;
        }
        let volumes = self
            .volumes
            .write_out(&mut self.io)?
            .ok_or(Error::Corrupt("empty volume table"))?;
        for (ptr, reusable) in core::mem::take(&mut self.io.freed) {
            if reusable {
                self.io.alloc.free_later(generation, run_of(&ptr));
            }
        }
        for ptr in core::mem::take(&mut self.alloc_chain) {
            self.io.alloc.free_later(generation, run_of(&ptr));
        }
        let (head, chain) = write_alloc(&mut self.io)?;
        if !self.faults.skip_data_flush {
            self.io.dev.flush()?;
        }
        let sb = Superblock {
            magic: format::MAGIC,
            format_major: format::FORMAT_MAJOR,
            format_minor: 0,
            compat: 0,
            ro_compat: 0,
            incompat: 0,
            uuid: self.uuid,
            generation,
            block_count: self.block_count,
            block_size: BLOCK as u32,
            csum_kind: self.io.kind as u8,
            pad: [0; 3],
            volumes,
            alloc: head,
            gc: BlockPtr::default(),
            keys: BlockPtr::default(),
            commit_time: generation,
            label: [0; 64],
        };
        let block = sb.encode(self.io.kind);
        for addr in format::slot_addrs(generation, self.block_count) {
            self.io.dev.write(addr, &block)?;
        }
        self.io.dev.flush()?;
        self.committed = generation;
        self.alloc_chain = chain;
        self.io.txn = generation + 1;
        let early = u64::from(self.faults.reuse_next);
        self.io.alloc.release(self.io.txn + early);
        Ok(generation)
    }

    pub fn snapshot_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.snaps.keys().copied()
    }

    pub fn live_root(&self) -> Option<BlockPtr> {
        match &self.live.root {
            Some(Child::Clean(ptr)) => Some(*ptr),
            _ => None,
        }
    }

    pub fn volumes_root(&self) -> Option<BlockPtr> {
        match &self.volumes.root {
            Some(Child::Clean(ptr)) => Some(*ptr),
            _ => None,
        }
    }

    pub fn alloc_chain(&self) -> &[BlockPtr] {
        &self.alloc_chain
    }

    /// Turns on [`Faults::ignore_snapshots`].
    pub fn set_ignore_snapshots(&mut self, on: bool) {
        self.faults.ignore_snapshots = on;
        self.io.ignore_snapshots = on;
    }

    /// Whether nothing has changed since the last commit.
    pub fn is_clean(&self) -> bool {
        !self.live_changed && !self.volumes.is_dirty() && self.io.freed.is_empty()
    }

    pub fn block_count(&self) -> u64 {
        self.block_count
    }
}

/// The allocator run a pointer's blocks occupy.
pub fn run_of(ptr: &BlockPtr) -> Run {
    Run {
        addr: ptr.addr,
        level: level_for(u64::from(ptr.blocks)),
    }
}

pub(crate) fn decode_value<D: BlockDev>(raw: &[u8], io: &Io<D>) -> Result<Value, Error> {
    match raw.first() {
        Some(&TAG_INLINE) => Ok(Value::Inline(raw[1..].to_vec())),
        Some(&TAG_RECORD) if raw.len() == RECORD_VALUE => {
            let (ptr, _) = BlockPtr::read_from_prefix(&raw[1..])
                .map_err(|_| Error::Corrupt("record value"))?;
            let len = u32::from_le_bytes(raw[65..69].try_into().unwrap()) as usize;
            io.read_record(&ptr, len).map(Value::Record)
        }
        _ => Err(Error::Corrupt("value tag")),
    }
}

/// The record a raw value points to, if any, with its length.
pub(crate) fn record_of(raw: &[u8]) -> Option<(BlockPtr, usize)> {
    if raw.first() != Some(&TAG_RECORD) || raw.len() != RECORD_VALUE {
        return None;
    }
    let (ptr, _) = BlockPtr::read_from_prefix(&raw[1..]).ok()?;
    Some((
        ptr,
        u32::from_le_bytes(raw[65..69].try_into().unwrap()) as usize,
    ))
}

fn read_volumes<D: BlockDev>(volumes: &Tree, io: &mut Io<D>) -> Result<BTreeMap<u64, Snap>, Error> {
    let mut out = BTreeMap::new();
    let Some(Child::Clean(root)) = &volumes.root else {
        return Err(Error::Corrupt("volume table"));
    };
    let mut stack = vec![*root];
    while let Some(ptr) = stack.pop() {
        match io.load(&ptr)? {
            Node::Leaf(items) => {
                for (key, value) in items {
                    out.insert(key, decode_volume(&value)?);
                }
            }
            Node::Branch(_, children) => {
                for (_, child) in children {
                    if let Child::Clean(ptr) = child {
                        stack.push(ptr);
                    }
                }
            }
        }
    }
    Ok(out)
}

// The allocator chain: blocks of 16-byte entries (a run, or a pending run and the
// generation that freed it), each block pointing to the next.
const ALLOC_HEADER: usize = 8 + 64;
const ALLOC_PER_BLOCK: usize = (BLOCK - ALLOC_HEADER) / 16;
const PENDING_BIT: u64 = 1 << 7;

fn alloc_entries(buddy: &Buddy) -> Vec<(u64, u64)> {
    let free = buddy
        .free_runs()
        .map(|run| (run.addr << 8 | u64::from(run.level), 0));
    let pending = buddy.pending.iter().map(|(generation, run)| {
        (
            run.addr << 8 | u64::from(run.level) | PENDING_BIT,
            *generation,
        )
    });
    free.chain(pending).collect()
}

fn write_alloc<D: BlockDev>(io: &mut Io<D>) -> Result<(BlockPtr, Vec<BlockPtr>), Error> {
    // Allocating the chain changes what it records, so grow it until it fits.
    let mut runs: Vec<Run> = Vec::new();
    let entries = loop {
        let entries = alloc_entries(&io.alloc);
        if entries.len().div_ceil(ALLOC_PER_BLOCK).max(1) <= runs.len() {
            break entries;
        }
        runs.push(io.alloc.alloc(0).ok_or(Error::NoSpace)?);
    };
    let mut next = BlockPtr::default();
    let mut chain = Vec::with_capacity(runs.len());
    let chunks: Vec<&[(u64, u64)]> = entries.chunks(ALLOC_PER_BLOCK).collect();
    for (i, run) in runs.iter().enumerate().rev() {
        let part = chunks.get(i).copied().unwrap_or(&[]);
        let mut block = [0; BLOCK];
        block[0] = 1;
        block[4..8].copy_from_slice(&(part.len() as u32).to_le_bytes());
        block[8..ALLOC_HEADER].copy_from_slice(next.as_bytes());
        for (j, (word, generation)) in part.iter().enumerate() {
            let at = ALLOC_HEADER + 16 * j;
            block[at..at + 8].copy_from_slice(&word.to_le_bytes());
            block[at + 8..at + 16].copy_from_slice(&generation.to_le_bytes());
        }
        io.dev.write(run.addr, &block)?;
        next = BlockPtr {
            addr: run.addr,
            birth: io.txn,
            blocks: 1,
            kind: KIND_ALLOC,
            csum: io.kind.sum(&block),
            ..BlockPtr::default()
        };
        chain.push(next);
    }
    chain.reverse();
    Ok((next, chain))
}

fn load_alloc<D: BlockDev>(io: &Io<D>, head: &BlockPtr) -> Result<(Buddy, Vec<BlockPtr>), Error> {
    let (mut free, mut pending, mut chain) = (Vec::new(), Vec::new(), Vec::new());
    let mut ptr = *head;
    let mut seen = BTreeSet::new();
    while !ptr.is_null() {
        if ptr.kind != KIND_ALLOC || !seen.insert(ptr.addr) {
            return Err(Error::Corrupt("allocator chain"));
        }
        let block = io.read_verified(&ptr, "allocator block")?;
        let count = u32::from_le_bytes(block[4..8].try_into().unwrap()) as usize;
        if block[0] != 1 || count > ALLOC_PER_BLOCK {
            return Err(Error::Corrupt("allocator block"));
        }
        for j in 0..count {
            let at = ALLOC_HEADER + 16 * j;
            let word = u64::from_le_bytes(block[at..at + 8].try_into().unwrap());
            let generation = u64::from_le_bytes(block[at + 8..at + 16].try_into().unwrap());
            let run = Run {
                addr: word >> 8,
                level: (word & 0x0f) as u8,
            };
            if run.level as usize >= crate::alloc_buddy::LEVELS {
                return Err(Error::Corrupt("allocator run"));
            }
            if word & PENDING_BIT != 0 {
                pending.push((generation, run));
            } else {
                free.push(run);
            }
        }
        chain.push(ptr);
        ptr = BlockPtr::read_from_prefix(&block[8..ALLOC_HEADER])
            .map_err(|_| Error::Corrupt("allocator chain"))?
            .0;
    }
    Ok((Buddy::from_parts(free, pending), chain))
}
