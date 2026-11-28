//! A fake DMA allocator: frames are 512-entry tables in a map, handed out
//! zeroed, with a limit to run out on. A retired frame leaves the map, so any
//! read or write of it (or of a frame it never gave) is a panic. It is reused
//! only after [`FakeDma::release_retired`], which stands for the caller's
//! invalidation and fills it with poison first, as a reused frame would hold
//! someone else's data.

#![allow(dead_code)]

use amdvi::pagetable::{DmaAlloc, ENTRIES};
use std::collections::BTreeMap;

/// What a released frame holds until it is given again: a present PDE to
/// nowhere, so a walk through a stale frame would show.
pub const POISON: u64 = 0x7000_dead_beef_f601;

pub struct FakeDma {
    pub frames: BTreeMap<u64, Box<[u64; ENTRIES]>>,
    /// Retired, not yet released: the IOMMU may still walk them.
    pub retired: Vec<u64>,
    /// Released, poisoned, and given again first.
    pub free: Vec<(u64, Box<[u64; ENTRIES]>)>,
    next: u64,
    /// Frames it may still give.
    pub left: usize,
}

impl FakeDma {
    pub fn new() -> Self {
        Self::with_frames(usize::MAX)
    }

    pub fn with_frames(left: usize) -> Self {
        Self {
            frames: BTreeMap::new(),
            retired: Vec::new(),
            free: Vec::new(),
            next: 0x10_0000,
            left,
        }
    }

    /// Frames in use: given and not retired.
    pub fn live(&self) -> usize {
        self.frames.len()
    }

    /// The invalidation has completed: the retired frames may be reused.
    pub fn release_retired(&mut self) {
        for phys in self.retired.drain(..) {
            self.free.push((phys, Box::new([POISON; ENTRIES])));
        }
    }
}

impl DmaAlloc for FakeDma {
    fn alloc(&mut self) -> Option<u64> {
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        let phys = match self.free.pop() {
            Some((phys, _poisoned)) => phys,
            None => {
                self.next += 4096;
                self.next
            }
        };
        self.frames.insert(phys, Box::new([0; ENTRIES]));
        Some(phys)
    }

    fn retire(&mut self, phys: u64) {
        self.frames
            .remove(&phys)
            .expect("a frame it gave, retired once");
        self.retired.push(phys);
    }

    fn read(&self, phys: u64, index: usize) -> u64 {
        self.frames.get(&phys).expect("a live frame")[index]
    }

    fn write(&mut self, phys: u64, index: usize, value: u64) {
        self.frames.get_mut(&phys).expect("a live frame")[index] = value;
    }
}
