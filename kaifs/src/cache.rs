//! A block cache for the `fs` server (ADR 0004 §7; P2 task 8): CLOCK eviction
//! (mm/src/reclaim.rs's method) over any [`BlockDev`], capped by a block count;
//! 16,384 blocks is the plan's 64 MiB.
//!
//! It is **write-through**: every write goes to the device before the call
//! returns, and a flush is the device's flush. So the device sees exactly the
//! writes and flushes kaifs makes, in the same order, and every crash gate's
//! result holds over a cached device unchanged; `tests/cache.rs` shows the
//! logs are identical. A write the device refuses leaves nothing of it cached.
//! Reads are served from the cache when they can be; written blocks are cached
//! too, since a commit's nodes are read again soon after.

use crate::dev::{BlockDev, DevInfo, IoError};
use crate::{BLOCK, Block};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// The plan's cap: 64 MiB of 4 KiB blocks.
pub const DEFAULT_BLOCKS: usize = (64 << 20) / BLOCK;

struct Slot {
    addr: u64,
    block: Box<Block>,
    referenced: bool,
}

/// `dev` behind a CLOCK cache of up to `capacity` blocks.
pub struct Cached<D> {
    pub dev: D,
    capacity: usize,
    slots: Vec<Slot>,
    index: BTreeMap<u64, usize>,
    hand: usize,
    pub hits: u64,
    pub misses: u64,
}

impl<D: BlockDev> Cached<D> {
    /// A cache of `capacity` blocks, at least one.
    pub fn new(dev: D, capacity: usize) -> Self {
        Cached {
            dev,
            capacity: capacity.max(1),
            slots: Vec::new(),
            index: BTreeMap::new(),
            hand: 0,
            hits: 0,
            misses: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Keeps `data` as block `addr`, evicting by CLOCK when full.
    fn keep(&mut self, addr: u64, data: &Block) {
        if let Some(&at) = self.index.get(&addr) {
            let slot = &mut self.slots[at];
            slot.block.copy_from_slice(data);
            slot.referenced = true;
            return;
        }
        if self.slots.len() < self.capacity {
            self.index.insert(addr, self.slots.len());
            self.slots.push(Slot {
                addr,
                block: Box::new(*data),
                referenced: true,
            });
            return;
        }
        // Each block passed over loses its second chance; the first without one
        // goes.
        while self.slots[self.hand].referenced {
            self.slots[self.hand].referenced = false;
            self.hand = (self.hand + 1) % self.slots.len();
        }
        let slot = &mut self.slots[self.hand];
        self.index.remove(&slot.addr);
        self.index.insert(addr, self.hand);
        slot.addr = addr;
        slot.block.copy_from_slice(data);
        slot.referenced = true;
        self.hand = (self.hand + 1) % self.slots.len();
    }

    /// Forgets block `addr`: the device's copy is all there is.
    fn forget(&mut self, addr: u64) {
        let Some(at) = self.index.remove(&addr) else {
            return;
        };
        let last = self.slots.len() - 1;
        self.slots.swap(at, last);
        self.slots.pop();
        if at < self.slots.len() {
            self.index.insert(self.slots[at].addr, at);
        }
        if self.hand >= self.slots.len() {
            self.hand = 0;
        }
    }
}

impl<D: BlockDev> BlockDev for Cached<D> {
    fn info(&self) -> DevInfo {
        self.dev.info()
    }

    fn read(&mut self, addr: u64, buf: &mut Block) -> Result<(), IoError> {
        if let Some(&at) = self.index.get(&addr) {
            let slot = &mut self.slots[at];
            buf.copy_from_slice(&slot.block[..]);
            slot.referenced = true;
            self.hits += 1;
            return Ok(());
        }
        self.misses += 1;
        self.dev.read(addr, buf)?;
        self.keep(addr, buf);
        Ok(())
    }

    fn write(&mut self, addr: u64, data: &Block) -> Result<(), IoError> {
        match self.dev.write(addr, data) {
            Ok(()) => {
                self.keep(addr, data);
                Ok(())
            }
            Err(e) => {
                self.forget(addr);
                Err(e)
            }
        }
    }

    fn flush(&mut self) -> Result<(), IoError> {
        self.dev.flush()
    }
}
