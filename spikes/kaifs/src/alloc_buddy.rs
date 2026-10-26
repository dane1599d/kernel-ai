//! The buddy allocator (ADR 0004 §1): levels 0-9, so a run of 2^level blocks
//! (4 KiB to 2 MiB) is naturally aligned. Freed runs wait in `pending` until the
//! commit two after the one that freed them (§3's N+2 rule). The spike writes its
//! own buddy instead of adapting RedoxFS's; kaifs proper adapts RedoxFS's, with
//! its MIT notice, as the ADR says.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

pub const LEVELS: usize = 10;

/// A run: its first block and its level (2^level blocks).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Run {
    pub addr: u64,
    pub level: u8,
}

impl Run {
    pub fn blocks(self) -> u64 {
        1 << self.level
    }

    pub fn end(self) -> u64 {
        self.addr + self.blocks()
    }
}

/// The level whose runs hold `blocks` blocks.
pub fn level_for(blocks: u64) -> u8 {
    blocks.max(1).next_power_of_two().trailing_zeros() as u8
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Buddy {
    free: [BTreeSet<u64>; LEVELS],
    /// Runs freed by generation `gen`, reusable from generation `gen + 2`.
    pub pending: Vec<(u64, Run)>,
}

impl Buddy {
    /// Everything in `[start, end)` free, in the largest aligned runs that fit.
    pub fn new(start: u64, end: u64) -> Self {
        let mut buddy = Self::default();
        let mut addr = start;
        while addr < end {
            let mut level = (LEVELS - 1) as u8;
            while level > 0 && (!addr.is_multiple_of(1 << level) || addr + (1 << level) > end) {
                level -= 1;
            }
            buddy.free[level as usize].insert(addr);
            addr += 1 << level;
        }
        buddy
    }

    /// A free run of `level`, split from a larger one if need be.
    pub fn alloc(&mut self, level: u8) -> Option<Run> {
        let from = (level as usize..LEVELS).find(|&l| !self.free[l].is_empty())?;
        let addr = self.free[from].pop_first()?;
        let mut l = from;
        while l > level as usize {
            l -= 1;
            self.free[l].insert(addr + (1 << l));
        }
        Some(Run { addr, level })
    }

    /// Returns `run` to the free sets at once, merging buddies.
    pub fn free_now(&mut self, run: Run) {
        let (mut addr, mut level) = (run.addr, run.level as usize);
        while level + 1 < LEVELS {
            let buddy = addr ^ (1 << level);
            if !self.free[level].remove(&buddy) {
                break;
            }
            addr = addr.min(buddy);
            level += 1;
        }
        self.free[level].insert(addr);
    }

    /// Frees `run` in generation `gen`; it becomes reusable at `gen + 2`.
    pub fn free_later(&mut self, generation: u64, run: Run) {
        self.pending.push((generation, run));
    }

    /// Releases every pending run whose wait is over by generation `gen`.
    pub fn release(&mut self, generation: u64) {
        let (ready, wait): (Vec<_>, Vec<_>) = self
            .pending
            .drain(..)
            .partition(|(freed, _)| freed + 2 <= generation);
        self.pending = wait;
        for (_, run) in ready {
            self.free_now(run);
        }
    }

    /// Every free run, lowest address first within a level.
    pub fn free_runs(&self) -> impl Iterator<Item = Run> + '_ {
        self.free.iter().enumerate().flat_map(|(level, set)| {
            set.iter().map(move |&addr| Run {
                addr,
                level: level as u8,
            })
        })
    }

    pub fn free_blocks(&self) -> u64 {
        self.free_runs().map(Run::blocks).sum()
    }

    /// Rebuilds the allocator from saved runs.
    pub fn from_parts(free: impl IntoIterator<Item = Run>, pending: Vec<(u64, Run)>) -> Self {
        let mut buddy = Self::default();
        for run in free {
            buddy.free[run.level as usize].insert(run.addr);
        }
        buddy.pending = pending;
        buddy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_are_aligned_and_merge_back() {
        let mut buddy = Buddy::new(9, 1024 - 8);
        let total = buddy.free_blocks();
        assert_eq!(total, 1024 - 8 - 9);
        let a = buddy.alloc(3).unwrap();
        assert_eq!(a.addr % 8, 0);
        let b = buddy.alloc(0).unwrap();
        assert_eq!(buddy.free_blocks(), total - 9);
        buddy.free_now(a);
        buddy.free_now(b);
        assert_eq!(buddy.free_blocks(), total);
    }

    #[test]
    fn pending_waits_two_generations() {
        let mut buddy = Buddy::new(16, 32);
        let run = buddy.alloc(0).unwrap();
        buddy.free_later(5, run);
        buddy.release(6);
        assert_eq!(buddy.pending.len(), 1);
        buddy.release(7);
        assert!(buddy.pending.is_empty());
        assert_eq!(buddy.free_blocks(), 16);
    }

    #[test]
    fn level_for_rounds_up() {
        assert_eq!(level_for(1), 0);
        assert_eq!(level_for(2), 1);
        assert_eq!(level_for(3), 2);
        assert_eq!(level_for(32), 5);
        assert_eq!(level_for(512), 9);
    }
}
