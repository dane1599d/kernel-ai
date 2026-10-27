//! Everything below the trees (ADR 0004 §1, §3): the disk, checksums, the
//! allocator, data records and values, and the reuse rule for replaced blocks,
//! with the stores the trees see the disk through.

use crate::Error;
use crate::allocator::{Allocator, run_of};
use crate::buddy::level_for;
use crate::csum::CsumKind;
use crate::dev::BlockDev;
use crate::format::{self, BlockPtr, FLAG_SHARED, KIND_NODE, KIND_RECORD, PTR_BYTES};
use crate::tree::{MAX_VALUE, Node, Store};
use crate::{BLOCK, Block};
use alloc::vec;
use alloc::vec::Vec;

/// The largest data record: 128 KiB (ADR 0004 §1; sealed 2 MiB blobs come with
/// task 10's model store).
pub const MAX_RECORD: usize = 128 << 10;
/// The largest inline value: a tree value less its tag byte.
pub const MAX_INLINE: usize = MAX_VALUE - 1;

const TAG_INLINE: u8 = 0;
pub(crate) const TAG_RECORD: u8 = 1;
pub(crate) const RECORD_VALUE: usize = 1 + PTR_BYTES + 4;

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
    /// An action that does not count as a snapshot while it is open, so its
    /// undo snapshot can hold blocks already freed.
    pub unpinned_action: bool,
}

/// Everything below the trees: the disk, checksums, the allocator and the rule
/// for replaced blocks.
pub struct Io<D> {
    pub dev: D,
    pub kind: CsumKind,
    pub alloc: Allocator,
    /// The generation being built: the last commit's plus one.
    pub txn: u64,
    /// Nodes read since mount, so tests can see what a walk cost.
    pub node_reads: u64,
    pub(crate) faults: Faults,
    /// Replaced blocks the faulty rule judges at commit, with their volume.
    pub(crate) deferred: Vec<(BlockPtr, u64)>,
}

impl<D: BlockDev> Io<D> {
    pub fn load_node(&mut self, ptr: &BlockPtr) -> Result<Node, Error> {
        if ptr.kind != KIND_NODE {
            return Err(Error::Corrupt("node pointer kind"));
        }
        self.node_reads += 1;
        let block = format::read_checked(&mut self.dev, self.kind, ptr, "node")?;
        let node = Node::decode(&block)?;
        match node.level() == ptr.level {
            true => Ok(node),
            false => Err(Error::Corrupt("node level")),
        }
    }

    pub(crate) fn store_node(&mut self, block: &Block, level: u8) -> Result<BlockPtr, Error> {
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

    /// Whether a block can be freed now, its volume's newest snapshot (or open
    /// action) being of generation `newest`.
    pub(crate) fn reusable(&self, ptr: &BlockPtr, newest: u64) -> bool {
        ptr.flags & FLAG_SHARED == 0 && (ptr.birth > newest || self.faults.ignore_snapshots)
    }

    /// A block of volume `vol` replaced in this transaction: freed if no snapshot
    /// can hold it now; else it waits for the GC.
    pub(crate) fn replaced_volume_block(&mut self, ptr: BlockPtr, newest: u64, vol: u64) {
        if self.faults.reuse_now {
            self.alloc.free_now(run_of(&ptr));
        } else if self.faults.decide_at_commit {
            self.deferred.push((ptr, vol));
        } else if self.reusable(&ptr, newest) {
            self.alloc.free_later(self.txn, run_of(&ptr));
        }
    }

    /// Writes `data` as a record to a free run and returns its pointer.
    pub(crate) fn write_record(&mut self, data: &[u8]) -> Result<BlockPtr, Error> {
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

/// A volume's trees' view of the disk: replaced blocks follow the reuse rule
/// with the volume's newest snapshot, or its open action.
pub(crate) struct Volume<'a, D> {
    pub(crate) io: &'a mut Io<D>,
    pub(crate) newest: u64,
    pub(crate) vol: u64,
}

/// The volume table's: its replaced blocks are always freed.
pub(crate) struct Meta<'a, D>(pub(crate) &'a mut Io<D>);

impl<D: BlockDev> Store for Volume<'_, D> {
    fn load(&mut self, ptr: &BlockPtr) -> Result<Node, Error> {
        self.io.load_node(ptr)
    }

    fn replaced(&mut self, ptr: BlockPtr) {
        self.io.replaced_volume_block(ptr, self.newest, self.vol);
    }

    fn store(&mut self, block: &Block, level: u8) -> Result<BlockPtr, Error> {
        self.io.store_node(block, level)
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
