//! The virtio bounce pool (P6 G66): fixed-size slots of physical memory that a
//! buffer the device cannot reach in place is copied through.
//!
//! `virtio_drivers::Hal::share` returns a bare physical address, and the queue
//! notifies the device right after it, so a bounce that finds no memory has no
//! safe way to fail. So the memory is set aside first: each virtio wrapper, at
//! init, adds as many slots as its queues can hold shared buffers at once, and a
//! wrapper that cannot get them fails its init instead. A share then takes a slot
//! and an unshare returns it; neither allocates, and while every queue stays
//! within the slots it added, a slot is always free.
//!
//! The pool only does the bookkeeping, over physical addresses; the kernel owns
//! the frames behind them and copies through the direct map.

use alloc::vec::Vec;

/// Bytes in one slot: room for the largest buffer any virtio wrapper bounces
/// (virtio-net's 2048-byte receive buffers), two to a page.
pub const SLOT_BYTES: u64 = 2048;

/// Why the pool refused a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BounceError {
    /// A run of slots that is empty, not slot-aligned, overlaps the pool's, or
    /// lies past 2^64.
    BadRun,
    /// The pool's own lists could not grow to track more slots.
    NoRoom,
    /// An address that is not the start of one of the pool's slots.
    NotASlot,
    /// A slot returned that was not taken.
    NotTaken,
}

/// One contiguous run of slots, added at once.
#[derive(Clone, Copy, Debug)]
struct Run {
    base: u64,
    slots: u64,
    /// The index of this run's first slot in `taken`.
    first: usize,
}

/// The slots, which are free, and which are taken.
#[derive(Debug, Default)]
pub struct BouncePool {
    runs: Vec<Run>,
    /// Free slots' addresses; its capacity is every slot's, so returning a slot
    /// never allocates.
    free: Vec<u64>,
    /// Whether each slot, by index across the runs, is taken.
    taken: Vec<bool>,
}

impl BouncePool {
    pub const fn new() -> Self {
        BouncePool {
            runs: Vec::new(),
            free: Vec::new(),
            taken: Vec::new(),
        }
    }

    /// Adds `slots` slots at `base..base + slots * SLOT_BYTES`, memory the caller
    /// has set aside for the pool for good. Refused (and nothing added) for a run
    /// that is empty, unaligned or overlaps another, or when the lists cannot grow.
    pub fn add(&mut self, base: u64, slots: u64) -> Result<(), BounceError> {
        let end = slots
            .checked_mul(SLOT_BYTES)
            .and_then(|len| base.checked_add(len))
            .ok_or(BounceError::BadRun)?;
        if slots == 0 || !base.is_multiple_of(SLOT_BYTES) {
            return Err(BounceError::BadRun);
        }
        let overlaps = self
            .runs
            .iter()
            .any(|run| base < run.base + run.slots * SLOT_BYTES && run.base < end);
        if overlaps {
            return Err(BounceError::BadRun);
        }
        let count = usize::try_from(slots).map_err(|_| BounceError::NoRoom)?;
        let total = self.taken.len() + count;
        self.runs.try_reserve(1).map_err(|_| BounceError::NoRoom)?;
        self.free
            .try_reserve_exact(total - self.free.len())
            .map_err(|_| BounceError::NoRoom)?;
        self.taken
            .try_reserve_exact(count)
            .map_err(|_| BounceError::NoRoom)?;
        self.runs.push(Run {
            base,
            slots,
            first: self.taken.len(),
        });
        self.taken.resize(total, false);
        // Lowest address on top, so a fresh pool hands out its slots in order.
        self.free
            .extend((0..slots).rev().map(|slot| base + slot * SLOT_BYTES));
        Ok(())
    }

    /// A free slot's address, now taken; `None` when every slot is.
    pub fn take(&mut self) -> Option<u64> {
        let phys = self.free.pop()?;
        let index = self.index(phys)?;
        self.taken[index] = true;
        Some(phys)
    }

    /// Returns the slot at `phys`, which [`take`](Self::take) handed out.
    pub fn give(&mut self, phys: u64) -> Result<(), BounceError> {
        let index = self.index(phys).ok_or(BounceError::NotASlot)?;
        if !self.taken[index] {
            return Err(BounceError::NotTaken);
        }
        self.taken[index] = false;
        // Within capacity: there are never more free slots than slots.
        self.free.push(phys);
        Ok(())
    }

    /// Every slot, free or taken.
    pub fn slots(&self) -> usize {
        self.taken.len()
    }

    /// Slots free now.
    pub fn free(&self) -> usize {
        self.free.len()
    }

    /// The index of the slot starting at `phys`.
    fn index(&self, phys: u64) -> Option<usize> {
        self.runs.iter().find_map(|run| {
            let offset = phys.checked_sub(run.base)?;
            let slot = offset / SLOT_BYTES;
            (offset.is_multiple_of(SLOT_BYTES) && slot < run.slots)
                .then(|| run.first + slot as usize)
        })
    }
}

/// The slots a queue of `size` descriptors needs when each request it carries
/// shares at most `buffers` buffers: one per descriptor, and with indirect
/// descriptors, which put a whole request in one, that request's buffers plus
/// its descriptor list for each.
pub fn queue_slots(size: u64, buffers: u64, indirect: bool) -> u64 {
    match indirect && buffers > 1 {
        true => size * (buffers + 1),
        false => size,
    }
}

#[cfg(test)]
mod tests;
