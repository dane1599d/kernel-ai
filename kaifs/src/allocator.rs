//! The allocator: the buddy in memory, the N+2 rule for freed blocks, and its
//! state on disk (format 1), which the superblock's `alloc` root names.
//!
//! **Reuse** (ADR 0004 §3). A block freed in generation N waits in `pending` and
//! is free again from generation N+2, so commit N still mounts intact if slot N+1
//! tears. Whether a block may be freed at all (snapshots, SHARED) is decided by
//! the caller when the block is replaced; the allocator only keeps the wait.
//!
//! **On disk.** A chain of allocator blocks, newest first, each pointing to the
//! one before it. The oldest part is a *base*: free space as extents, and every
//! pending extent with the generation that freed it. After it, each commit that
//! allocated or freed anything adds a *delta*: the extents it allocated, and the
//! extents it freed. The wait needs no entries: a pending extent is free from its
//! generation plus two, so replay derives it, as the running allocator does.
//! When the chain holds [`MAX_DELTAS`] deltas, a commit writes a new base instead
//! and frees the old chain, so a mount reads a bounded chain and a commit
//! usually writes one block. Free space is stored as extents, so a fresh disk's
//! base is one entry whatever its size.
//!
//! Every block a commit allocates is in its own entries, the allocator's own
//! blocks included, so a mount rebuilds exactly the state the commit had.

use crate::buddy::{Buddy, Run, level_for, tile};
use crate::csum::CsumKind;
use crate::dev::BlockDev;
use crate::format::{self, BlockPtr, KIND_ALLOC, PTR_BYTES};
use crate::{BLOCK, Block, Error};
use alloc::collections::BTreeSet;
use alloc::vec::Vec;

/// Deltas a chain may hold before a commit writes a new base.
pub const MAX_DELTAS: u32 = 64;

const FMT: u8 = 1;
const BASE: u8 = 1;
const DELTA: u8 = 2;
const HEADER: usize = 16 + PTR_BYTES;
const ENTRY: usize = 24;
/// Entries in one allocator block.
pub const PER_BLOCK: usize = (BLOCK - HEADER) / ENTRY;

/// What an entry says about its extent.
const OP_FREE: u8 = 1;
const OP_PENDING: u8 = 2;
const OP_ALLOC: u8 = 3;

/// Deliberate bugs, so the crash gates can show they catch each one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Faults {
    /// A delta that leaves out its own blocks' allocation.
    pub unlogged_self: bool,
    /// A new base that frees the old chain at once instead of after N+2.
    pub free_chain_now: bool,
    /// Freed blocks reused at N+1. Safe with two flushes a commit (ADR 0004 §3):
    /// the gates must find nothing.
    pub reuse_next: bool,
}

#[derive(Clone, Debug)]
pub struct Allocator {
    buddy: Buddy,
    /// Runs freed, with the generation that freed them.
    pending: Vec<(u64, Run)>,
    /// This transaction's allocations and frees, for its delta.
    allocs: Vec<Run>,
    frees: Vec<Run>,
    /// The chain the last commit wrote: its newest block, and all of its blocks.
    head: BlockPtr,
    chain: Vec<BlockPtr>,
    deltas: u32,
    pub faults: Faults,
}

/// What the allocator holds, to compare a mounted state with a committed one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub free: Vec<(u64, u64)>,
    pub pending: Vec<(u64, u64, u64)>,
}

/// `runs` as sorted, joined extents.
fn extents(runs: impl IntoIterator<Item = Run>) -> Vec<(u64, u64)> {
    let mut runs: Vec<Run> = runs.into_iter().collect();
    runs.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for run in runs {
        match out.last_mut() {
            Some((s, l)) if *s + *l == run.addr => *l += run.blocks(),
            _ => out.push((run.addr, run.blocks())),
        }
    }
    out
}

/// The run a pointer's blocks occupy.
pub fn run_of(ptr: &BlockPtr) -> Run {
    Run {
        addr: ptr.addr,
        level: level_for(u64::from(ptr.blocks)),
    }
}

impl Allocator {
    /// A fresh filesystem's: everything in `area` free, no chain yet.
    pub fn new(area: (u64, u64)) -> Self {
        Allocator {
            buddy: Buddy::new(area.0, area.1),
            pending: Vec::new(),
            allocs: Vec::new(),
            frees: Vec::new(),
            head: BlockPtr::NULL,
            chain: Vec::new(),
            deltas: 0,
            faults: Faults::default(),
        }
    }

    /// A free run of `level`, recorded for this transaction's delta.
    pub fn alloc(&mut self, level: u8) -> Result<Run, Error> {
        let run = self.buddy.alloc(level).ok_or(Error::NoSpace)?;
        self.allocs.push(run);
        Ok(run)
    }

    /// Frees `run` in generation `txn`: free again from `txn + 2`.
    pub fn free_later(&mut self, txn: u64, run: Run) {
        self.pending.push((txn, run));
        self.frees.push(run);
    }

    /// Frees `run` at once. Only for a run allocated in this transaction that no
    /// commit names, or for a deliberate fault.
    pub fn free_now(&mut self, run: Run) {
        self.buddy.free(run);
    }

    /// Releases every pending run whose wait ends by generation `txn`.
    fn release(&mut self, txn: u64) {
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].0 + 2 <= txn {
                let (_, run) = self.pending.swap_remove(i);
                self.buddy.free(run);
            } else {
                i += 1;
            }
        }
    }

    pub fn free_blocks(&self) -> u64 {
        self.buddy.free_blocks()
    }

    /// Whether any block of `[start, end)` is free or pending: what `check()`
    /// asks of every block a tree reaches.
    pub fn any_unused(&self, start: u64, end: u64) -> bool {
        self.buddy.any_free(start, end)
            || self
                .pending
                .iter()
                .any(|(_, r)| r.addr < end && r.end() > start)
    }

    /// Every block of the allocator's own chain.
    pub fn chain(&self) -> &[BlockPtr] {
        &self.chain
    }

    pub fn state(&self) -> State {
        let mut pending: Vec<(u64, u64, u64)> = Vec::new();
        let mut by_txn: Vec<(u64, Run)> = self.pending.clone();
        by_txn.sort_unstable();
        for (txn, run) in by_txn {
            match pending.last_mut() {
                Some((t, s, l)) if *t == txn && *s + *l == run.addr => *l += run.blocks(),
                _ => pending.push((txn, run.addr, run.blocks())),
            }
        }
        State {
            free: self.buddy.extents(),
            pending,
        }
    }

    /// Writes this transaction's part of the chain in generation `txn` and returns
    /// the chain's new head, for the superblock. No flush: the commit flushes.
    pub fn persist<D: BlockDev>(
        &mut self,
        dev: &mut D,
        kind: CsumKind,
        txn: u64,
    ) -> Result<BlockPtr, Error> {
        let condense = self.head.is_null() || self.deltas >= MAX_DELTAS;
        if !condense && self.allocs.is_empty() && self.frees.is_empty() {
            return Ok(self.head);
        }
        match condense {
            true => self.write_base(dev, kind, txn),
            false => self.write_delta(dev, kind, txn),
        }
    }

    /// Allocates blocks until `entries()` fits in them, counting again after each,
    /// since allocating changes what is to be recorded.
    fn grow(&mut self, entries: impl Fn(&Self) -> usize) -> Result<Vec<Run>, Error> {
        let mut blocks = Vec::new();
        while entries(self).div_ceil(PER_BLOCK).max(1) > blocks.len() {
            blocks.push(self.alloc(0)?);
        }
        Ok(blocks)
    }

    fn write_base<D: BlockDev>(
        &mut self,
        dev: &mut D,
        kind: CsumKind,
        txn: u64,
    ) -> Result<BlockPtr, Error> {
        // The old chain is not named once this commit lands.
        for ptr in core::mem::take(&mut self.chain) {
            match self.faults.free_chain_now {
                true => self.free_now(run_of(&ptr)),
                false => self.free_later(txn, run_of(&ptr)),
            }
        }
        let blocks = self.grow(|a| {
            let state = a.state();
            state.free.len() + state.pending.len()
        })?;
        let state = self.state();
        let mut entries: Vec<[u64; 3]> = Vec::new();
        entries.extend(state.free.iter().map(|&(s, l)| [s, l, u64::from(OP_FREE)]));
        entries.extend(
            state
                .pending
                .iter()
                .map(|&(t, s, l)| [s, l, t << 8 | u64::from(OP_PENDING)]),
        );
        self.write_chain(dev, kind, txn, BASE, &blocks, &entries, BlockPtr::NULL)?;
        self.deltas = 0;
        Ok(self.head)
    }

    fn write_delta<D: BlockDev>(
        &mut self,
        dev: &mut D,
        kind: CsumKind,
        txn: u64,
    ) -> Result<BlockPtr, Error> {
        let unlogged = self.faults.unlogged_self;
        let blocks = self.grow(|a| {
            extents(a.allocs.iter().copied()).len() + extents(a.frees.iter().copied()).len()
        })?;
        let allocs = self
            .allocs
            .iter()
            .filter(|r| !(unlogged && blocks.contains(r)))
            .copied();
        let mut entries: Vec<[u64; 3]> = Vec::new();
        entries.extend(
            extents(allocs)
                .iter()
                .map(|&(s, l)| [s, l, u64::from(OP_ALLOC)]),
        );
        entries.extend(
            extents(self.frees.iter().copied())
                .iter()
                .map(|&(s, l)| [s, l, txn << 8 | u64::from(OP_PENDING)]),
        );
        let prev = self.head;
        self.write_chain(dev, kind, txn, DELTA, &blocks, &entries, prev)?;
        self.deltas += 1;
        Ok(self.head)
    }

    /// Writes `entries` over `blocks`, linked newest first, the last pointing to
    /// `prev`, and makes the first the head.
    #[allow(clippy::too_many_arguments)]
    fn write_chain<D: BlockDev>(
        &mut self,
        dev: &mut D,
        kind: CsumKind,
        txn: u64,
        which: u8,
        blocks: &[Run],
        entries: &[[u64; 3]],
        prev: BlockPtr,
    ) -> Result<(), Error> {
        let mut next = prev;
        let mut written = Vec::with_capacity(blocks.len());
        for (i, run) in blocks.iter().enumerate().rev() {
            let part = entries.chunks(PER_BLOCK).nth(i).unwrap_or(&[]);
            let mut block = [0; BLOCK];
            block[0] = FMT;
            block[1] = which;
            block[4..8].copy_from_slice(&(part.len() as u32).to_le_bytes());
            block[8..16].copy_from_slice(&txn.to_le_bytes());
            block[16..HEADER].copy_from_slice(&next.encode());
            for (j, entry) in part.iter().enumerate() {
                let at = HEADER + j * ENTRY;
                for (k, word) in entry.iter().enumerate() {
                    block[at + 8 * k..at + 8 * k + 8].copy_from_slice(&word.to_le_bytes());
                }
            }
            dev.write(run.addr, &block)?;
            next = BlockPtr {
                addr: run.addr,
                birth: txn,
                blocks: 1,
                kind: KIND_ALLOC,
                level: 0,
                flags: 0,
                csum: kind.sum(&block),
            };
            written.push(next);
        }
        written.reverse();
        self.chain.extend(written);
        self.head = next;
        Ok(())
    }

    /// After commit `txn` is durable: the transaction's lists start again, and
    /// runs whose wait is over are free for the next transaction.
    pub fn committed(&mut self, txn: u64) {
        self.allocs.clear();
        self.frees.clear();
        let early = u64::from(self.faults.reuse_next);
        self.release(txn + 1 + early);
    }

    /// The allocator commit `generation` left, from the chain `head` names.
    pub fn load<D: BlockDev>(
        dev: &mut D,
        kind: CsumKind,
        head: &BlockPtr,
        generation: u64,
    ) -> Result<Self, Error> {
        Self::load_with(dev, kind, head, generation, Faults::default())
    }

    /// [`Allocator::load`] for a writer with `faults`: its reuse wait, too.
    pub fn load_with<D: BlockDev>(
        dev: &mut D,
        kind: CsumKind,
        head: &BlockPtr,
        generation: u64,
        faults: Faults,
    ) -> Result<Self, Error> {
        let early = u64::from(faults.reuse_next);
        let bad = Error::Corrupt("allocator chain");
        let area = format::data_area(dev.info().blocks);
        let limit = (MAX_DELTAS as usize + 1) * 64 + (area.1 - area.0) as usize / PER_BLOCK + 16;
        let mut blocks: Vec<(BlockPtr, alloc::boxed::Box<Block>)> = Vec::new();
        let mut seen = BTreeSet::new();
        let mut ptr = *head;
        while !ptr.is_null() {
            if ptr.kind != KIND_ALLOC || !seen.insert(ptr.addr) || blocks.len() > limit {
                return Err(bad);
            }
            let block = format::read_checked(dev, kind, &ptr, "allocator block")?;
            let next = BlockPtr::decode_at(&block[..], 16)?;
            blocks.push((ptr, block));
            ptr = next;
        }
        let mut alloc = Allocator::new(area);
        alloc.buddy = Buddy::default();
        let mut last: Option<(u8, u64)> = None;
        // Oldest first: the base, then each delta in turn.
        for (ptr, block) in blocks.iter().rev() {
            let (which, txn) = (
                block[1],
                u64::from_le_bytes(block[8..16].try_into().unwrap_or([0; 8])),
            );
            let count = u32::from_le_bytes(block[4..8].try_into().unwrap_or([0; 4])) as usize;
            let ordered = match last {
                None => which == BASE,
                Some((BASE, t)) => (which == BASE && txn == t) || (which == DELTA && txn > t),
                Some((_, t)) => which == DELTA && txn >= t,
            };
            if block[0] != FMT || count > PER_BLOCK || !ordered || txn > generation {
                return Err(bad);
            }
            if which == DELTA && last.is_some_and(|(_, t)| txn > t) {
                alloc.release(txn + early);
            }
            last = Some((which, txn));
            for j in 0..count {
                let at = HEADER + j * ENTRY;
                let word = |k: usize| {
                    u64::from_le_bytes(
                        block[at + 8 * k..at + 8 * k + 8]
                            .try_into()
                            .unwrap_or([0; 8]),
                    )
                };
                let (start, len, tag) = (word(0), word(1), word(2));
                let end = start.checked_add(len).ok_or(bad)?;
                if len == 0 || start < area.0 || end > area.1 {
                    return Err(bad);
                }
                match ((tag & 0xff) as u8, which) {
                    (OP_FREE, BASE) if tag >> 8 == 0 => {
                        if alloc.buddy.any_free(start, end) {
                            return Err(bad);
                        }
                        alloc.buddy.free_extent(start, len);
                    }
                    (OP_PENDING, BASE) | (OP_PENDING, DELTA)
                        if (which == BASE && tag >> 8 <= txn) || tag >> 8 == txn =>
                    {
                        if alloc.buddy.any_free(start, end) {
                            return Err(bad);
                        }
                        alloc
                            .pending
                            .extend(tile(start, end).map(|run| (tag >> 8, run)));
                    }
                    (OP_ALLOC, DELTA) if tag >> 8 == 0 => {
                        if !alloc.buddy.take_extent(start, len) {
                            return Err(bad);
                        }
                    }
                    _ => return Err(bad),
                }
            }
            alloc.chain.push(*ptr);
        }
        let mut pending: Vec<Run> = alloc.pending.iter().map(|(_, r)| *r).collect();
        pending.sort_unstable();
        if pending.windows(2).any(|w| w[0].end() > w[1].addr) {
            return Err(bad);
        }
        alloc.deltas = blocks
            .iter()
            .filter(|(_, b)| b[1] == DELTA)
            .map(|(_, b)| u64::from_le_bytes(b[8..16].try_into().unwrap_or([0; 8])))
            .collect::<BTreeSet<u64>>()
            .len() as u32;
        alloc.head = *head;
        alloc.faults = faults;
        alloc.release(generation + 1 + early);
        Ok(alloc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dev::MemDisk;

    #[test]
    fn a_fresh_disk_is_one_extent_and_one_block() {
        let mut disk = MemDisk::new(1 << 20);
        let area = format::data_area(1 << 20);
        let mut alloc = Allocator::new(area);
        let head = alloc.persist(&mut disk, CsumKind::Seahash64, 1).unwrap();
        alloc.committed(1);
        assert_eq!(alloc.chain().len(), 1);
        let loaded = Allocator::load(&mut disk, CsumKind::Seahash64, &head, 1).unwrap();
        assert_eq!(loaded.state(), alloc.state());
        // The base took the lowest block; the rest is still one extent.
        assert_eq!(loaded.state().free, [(area.0 + 1, area.1 - area.0 - 1)]);
    }

    #[test]
    fn deltas_replay_to_the_same_state_and_condense_after_max_deltas() {
        let mut disk = MemDisk::new(1 << 16);
        let area = format::data_area(1 << 16);
        let mut alloc = Allocator::new(area);
        let mut held = Vec::new();
        let mut head = BlockPtr::NULL;
        for txn in 1..=200u64 {
            for i in 0..(txn % 7) {
                held.push(alloc.alloc((i % 4) as u8).unwrap());
            }
            if txn % 3 == 0 {
                for _ in 0..held.len() / 2 {
                    let run = held.swap_remove(0);
                    alloc.free_later(txn, run);
                }
            }
            head = alloc.persist(&mut disk, CsumKind::Seahash64, txn).unwrap();
            alloc.committed(txn);
            let loaded = Allocator::load(&mut disk, CsumKind::Seahash64, &head, txn).unwrap();
            assert_eq!(loaded.state(), alloc.state(), "txn {txn}");
            assert!(alloc.chain().len() <= MAX_DELTAS as usize + 4);
        }
        assert!(!head.is_null());
    }
}
