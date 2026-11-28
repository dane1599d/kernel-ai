//! A fake DMA allocator: frames are 512-entry tables in a map, handed out
//! zeroed from 1 MiB up, with a limit to run out on, and every read or write
//! of a frame it did not give (or took back) a panic.

#![allow(dead_code)]

use amdvi::pagetable::{DmaAlloc, ENTRIES};
use std::collections::BTreeMap;

pub struct FakeDma {
    pub frames: BTreeMap<u64, Box<[u64; ENTRIES]>>,
    next: u64,
    /// Frames it may still give.
    pub left: usize,
    pub freed: usize,
}

impl FakeDma {
    pub fn new() -> Self {
        Self::with_frames(usize::MAX)
    }

    pub fn with_frames(left: usize) -> Self {
        Self {
            frames: BTreeMap::new(),
            next: 0x10_0000,
            left,
            freed: 0,
        }
    }

    /// Frames given and not freed.
    pub fn live(&self) -> usize {
        self.frames.len()
    }
}

impl DmaAlloc for FakeDma {
    fn alloc(&mut self) -> Option<u64> {
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        let phys = self.next;
        self.next += 4096;
        self.frames.insert(phys, Box::new([0; ENTRIES]));
        Some(phys)
    }

    fn free(&mut self, phys: u64) {
        self.frames.remove(&phys).expect("a frame it gave");
        self.freed += 1;
    }

    fn read(&self, phys: u64, index: usize) -> u64 {
        self.frames.get(&phys).expect("a live frame")[index]
    }

    fn write(&mut self, phys: u64, index: usize, value: u64) {
        self.frames.get_mut(&phys).expect("a live frame")[index] = value;
    }
}
