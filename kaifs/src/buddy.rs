//! The buddy allocator in memory (ADR 0004 §1): runs of 2^level blocks, levels 0
//! to 9 (4 KiB to 2 MiB), each aligned to its own size. Free runs below 2 MiB sit
//! in one set per level; whole 2 MiB runs sit as extents, so memory follows how
//! fragmented the disk is, not how large it is. Written for kaifs (S04-1's
//! design); RedoxFS's allocator, which ADR 0004 names, waits on ADR 0009's
//! question 4.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

/// The largest level: a 2 MiB run.
pub const TOP: u8 = 9;
const TOP_BLOCKS: u64 = 1 << TOP;

/// A run of `2^level` blocks starting at `addr`, which is a multiple of its size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

/// The level whose runs hold `blocks` blocks: the next power of two.
pub fn level_for(blocks: u64) -> u8 {
    blocks.max(1).next_power_of_two().trailing_zeros() as u8
}

/// The aligned runs that tile `[start, end)` exactly, each as large as its
/// position allows, in address order.
pub fn tile(start: u64, end: u64) -> impl Iterator<Item = Run> {
    let mut addr = start;
    core::iter::from_fn(move || {
        if addr >= end {
            return None;
        }
        let mut level = TOP;
        while level > 0 && (!addr.is_multiple_of(1 << level) || addr + (1 << level) > end) {
            level -= 1;
        }
        let run = Run { addr, level };
        addr = run.end();
        Some(run)
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Buddy {
    /// Free runs of levels 0-8, by address.
    small: [BTreeSet<u64>; TOP as usize],
    /// Whole free 2 MiB runs as extents: first block, then how many runs.
    big: BTreeMap<u64, u64>,
}

impl Buddy {
    /// Everything in `[start, end)` free.
    pub fn new(start: u64, end: u64) -> Self {
        let mut buddy = Self::default();
        for run in tile(start, end) {
            buddy.free(run);
        }
        buddy
    }

    /// A free run of `level`, the lowest at that size, split from a larger one if
    /// need be.
    pub fn alloc(&mut self, level: u8) -> Option<Run> {
        assert!(level <= TOP, "runs are at most 2 MiB");
        let (addr, from) = match (level..TOP).find(|&l| !self.small[l as usize].is_empty()) {
            Some(l) => (self.small[l as usize].pop_first()?, l),
            None => (self.take_top_first()?, TOP),
        };
        let run = Run { addr, level };
        self.split(addr, from, run);
        Some(run)
    }

    fn take_top_first(&mut self) -> Option<u64> {
        let (&start, &count) = self.big.iter().next()?;
        self.big.remove(&start);
        if count > 1 {
            self.big.insert(start + TOP_BLOCKS, count - 1);
        }
        Some(start)
    }

    /// A free run of `level` at `base` was taken to hold `target`: the halves that
    /// do not hold it go back, level by level.
    fn split(&mut self, mut base: u64, mut level: u8, target: Run) {
        while level > target.level {
            level -= 1;
            let half = 1u64 << level;
            let other = if target.addr >= base + half {
                let lower = base;
                base += half;
                lower
            } else {
                base + half
            };
            self.small[level as usize].insert(other);
        }
    }

    /// Returns `run` to free space, merging it with its buddies.
    pub fn free(&mut self, run: Run) {
        let (mut addr, mut level) = (run.addr, run.level);
        while level < TOP {
            let buddy = addr ^ (1 << level);
            if !self.small[level as usize].remove(&buddy) {
                break;
            }
            addr = addr.min(buddy);
            level += 1;
        }
        match level < TOP {
            true => {
                self.small[level as usize].insert(addr);
            }
            false => self.insert_top(addr, 1),
        }
    }

    fn insert_top(&mut self, mut start: u64, mut count: u64) {
        if let Some((&before, &n)) = self.big.range(..start).next_back()
            && before + n * TOP_BLOCKS == start
        {
            self.big.remove(&before);
            start = before;
            count += n;
        }
        let end = start + count * TOP_BLOCKS;
        if let Some(n) = self.big.remove(&end) {
            count += n;
        }
        self.big.insert(start, count);
    }

    /// Takes the 2 MiB run at `top` out of the big extents, if one holds it.
    fn remove_top(&mut self, top: u64) -> bool {
        let Some((&start, &count)) = self.big.range(..=top).next_back() else {
            return false;
        };
        if top >= start + count * TOP_BLOCKS {
            return false;
        }
        self.big.remove(&start);
        let before = (top - start) / TOP_BLOCKS;
        if before > 0 {
            self.big.insert(start, before);
        }
        if count - before > 1 {
            self.big.insert(top + TOP_BLOCKS, count - before - 1);
        }
        true
    }

    /// Takes `run`, which must lie inside one free run, out of free space; false
    /// if it is not free.
    pub fn take(&mut self, run: Run) -> bool {
        for level in run.level..TOP {
            let base = run.addr & !((1u64 << level) - 1);
            if self.small[level as usize].remove(&base) {
                self.split(base, level, run);
                return true;
            }
        }
        let top = run.addr & !(TOP_BLOCKS - 1);
        if self.remove_top(top) {
            self.split(top, TOP, run);
            return true;
        }
        false
    }

    /// Takes every block of `[start, start + len)`; false if one was not free.
    pub fn take_extent(&mut self, start: u64, len: u64) -> bool {
        tile(start, start + len).all(|run| self.take(run))
    }

    /// Returns every block of `[start, start + len)` to free space.
    pub fn free_extent(&mut self, start: u64, len: u64) {
        for run in tile(start, start + len) {
            self.free(run);
        }
    }

    /// Whether any block of `[start, end)` is free.
    pub fn any_free(&self, start: u64, end: u64) -> bool {
        let small = self.small.iter().enumerate().any(|(level, set)| {
            let from = (start + 1).saturating_sub(1 << level);
            set.range(from..end).next().is_some()
        });
        let big = self
            .big
            .range(..end)
            .next_back()
            .is_some_and(|(&s, &n)| s + n * TOP_BLOCKS > start);
        small || big
    }

    /// Free space as extents, in address order, adjacent runs joined.
    pub fn extents(&self) -> Vec<(u64, u64)> {
        let mut runs: Vec<(u64, u64)> = self
            .small
            .iter()
            .enumerate()
            .flat_map(|(level, set)| set.iter().map(move |&addr| (addr, 1u64 << level)))
            .chain(self.big.iter().map(|(&s, &n)| (s, n * TOP_BLOCKS)))
            .collect();
        runs.sort_unstable();
        let mut out: Vec<(u64, u64)> = Vec::with_capacity(runs.len());
        for (start, len) in runs {
            match out.last_mut() {
                Some((s, l)) if *s + *l == start => *l += len,
                _ => out.push((start, len)),
            }
        }
        out
    }

    pub fn free_blocks(&self) -> u64 {
        let small: u64 = self
            .small
            .iter()
            .enumerate()
            .map(|(level, set)| set.len() as u64 * (1 << level))
            .sum();
        small + self.big.values().map(|n| n * TOP_BLOCKS).sum::<u64>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeSet;

    struct Rng(u64);
    impl Rng {
        fn below(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    #[test]
    fn tiles_are_aligned_and_exact() {
        let runs: Vec<Run> = tile(9, 4096 - 8).collect();
        assert!(runs.iter().all(|r| r.addr.is_multiple_of(r.blocks())));
        assert!(runs.windows(2).all(|w| w[0].end() == w[1].addr));
        assert_eq!((runs[0].addr, runs.last().unwrap().end()), (9, 4088));
        assert_eq!(runs.iter().map(|r| r.blocks()).sum::<u64>(), 4088 - 9);
    }

    #[test]
    fn a_large_disk_costs_one_extent_until_it_fragments() {
        let buddy = Buddy::new(9, (1 << 28) - 8);
        assert_eq!(buddy.big.len(), 1);
        assert_eq!(buddy.extents(), [(9, (1 << 28) - 8 - 9)]);
    }

    #[test]
    fn level_for_rounds_up() {
        assert_eq!([1, 2, 3, 32, 512].map(level_for), [0, 1, 2, 5, 9]);
    }

    /// Random allocations, frees and takes against a model of the free blocks.
    #[test]
    fn random_operations_agree_with_a_model() {
        let (start, end) = (9u64, 9000u64);
        let mut buddy = Buddy::new(start, end);
        let mut free: BTreeSet<u64> = (start..end).collect();
        let mut held: Vec<Run> = Vec::new();
        let mut rng = Rng(0x5EED);
        for step in 0..20_000 {
            match rng.below(10) {
                0..=4 => {
                    let level = [0, 0, 0, 1, 2, 3, 5, 9][rng.below(8) as usize];
                    if let Some(run) = buddy.alloc(level) {
                        assert!(run.addr.is_multiple_of(run.blocks()), "step {step}");
                        for b in run.addr..run.end() {
                            assert!(free.remove(&b), "step {step}: block {b} given twice");
                        }
                        held.push(run);
                    } else {
                        assert!(
                            !(start..end).step_by(1 << level).any(|a| {
                                let a = a.next_multiple_of(1 << level);
                                a + (1 << level) <= end
                                    && (a..a + (1 << level)).all(|b| free.contains(&b))
                            }),
                            "step {step}: level {level} refused while a run was free"
                        );
                    }
                }
                5..=8 if !held.is_empty() => {
                    let run = held.swap_remove(rng.below(held.len() as u64) as usize);
                    buddy.free(run);
                    free.extend(run.addr..run.end());
                }
                _ => {
                    // Take a specific free block, as a replayed allocation does.
                    if let Some(&b) = free
                        .iter()
                        .nth(rng.below(free.len().max(1) as u64) as usize)
                    {
                        assert!(buddy.take(Run { addr: b, level: 0 }));
                        free.remove(&b);
                        held.push(Run { addr: b, level: 0 });
                    }
                }
            }
            if step % 500 == 0 {
                let mut model: Vec<(u64, u64)> = Vec::new();
                for &b in &free {
                    match model.last_mut() {
                        Some((s, l)) if *s + *l == b => *l += 1,
                        _ => model.push((b, 1)),
                    }
                }
                assert_eq!(buddy.extents(), model, "step {step}");
                assert_eq!(buddy.free_blocks(), free.len() as u64);
            }
        }
        for run in held.drain(..) {
            buddy.free(run);
        }
        assert_eq!(buddy, Buddy::new(start, end), "everything merges back");
    }

    #[test]
    fn take_refuses_what_is_not_free_and_any_free_sees_every_level() {
        let mut buddy = Buddy::new(512, 512 * 4);
        let run = buddy.alloc(3).unwrap();
        assert!(!buddy.take(run));
        assert!(!buddy.any_free(run.addr, run.end()));
        assert!(buddy.any_free(run.addr, run.end() + 1));
        assert!(buddy.take_extent(1024, 700));
        assert!(!buddy.take_extent(1024, 1));
        assert!(!buddy.any_free(1024, 1724));
        buddy.free_extent(1024, 700);
        assert!(buddy.any_free(1500, 1501));
    }
}
