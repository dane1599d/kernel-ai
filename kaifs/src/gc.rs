//! The GC and retention (ADR 0004 §3-4; P2 task 6).
//!
//! **The GC** is mark-and-sweep with no reference counts (HAMMER2's method). A
//! pass marks from the filesystem as one commit left it: every volume's live tree
//! and snapshots, the volume table and the allocator's chain. It runs in slices
//! of a given number of nodes ([`Fs::gc_step`]), so the `fs` server can run one
//! between requests in at most 5 ms. While a pass runs it counts as a snapshot of
//! its generation, for volume and volume-table blocks alike, so no block it will
//! read is freed under it; blocks replaced meanwhile wait for the next pass. When nothing is left to mark, every block
//! that was in use then and that nothing reached is freed, reusable two commits
//! later, as any freed block is.
//!
//! **Retention.** [`Fs::expire`] drops the undo snapshots whose 30 days are up,
//! with their UNDO records, and, when free space is under a tenth of the data
//! area, the oldest one left. It returns what it dropped, for the log; the space
//! comes back with the next GC pass.

use crate::Error;
use crate::allocator::run_of;
use crate::buddy::{Run, tile};
use crate::dev::BlockDev;
use crate::format::{self, BlockPtr};
use crate::fs::{Fs, record_of};
use crate::tree::{Child, Node};
use crate::vol::SnapKind;
use alloc::collections::BTreeSet;
use alloc::vec;
use alloc::vec::Vec;

/// A pass in progress.
pub struct Gc {
    /// The generation it marks from.
    pub(crate) generation: u64,
    /// Nodes still to read, and whether their leaves hold values with records.
    stack: Vec<(BlockPtr, bool)>,
    seen: BTreeSet<u64>,
    marked: Vec<Run>,
    /// What was in use when it began, as extents.
    used: Vec<(u64, u64)>,
    nodes: u64,
}

/// How far a pass has got.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GcProgress {
    /// The pass is over, and what it freed is pending.
    pub done: bool,
    pub nodes: u64,
    pub freed_blocks: u64,
}

/// Why retention dropped a snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// Its 30 days were up.
    Expired,
    /// Free space was under a tenth, and it was the oldest undo snapshot.
    Space,
}

/// One undo snapshot retention dropped, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dropped {
    pub volume: u64,
    pub snapshot: u64,
    pub action: u64,
    pub why: Why,
}

/// Sorted, joined extents of `runs`.
fn extents(mut runs: Vec<Run>) -> Vec<(u64, u64)> {
    runs.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for run in runs {
        match out.last_mut() {
            Some((s, l)) if *s + *l >= run.addr => *l = (*l).max(run.end() - *s),
            _ => out.push((run.addr, run.blocks())),
        }
    }
    out
}

/// The parts of sorted extents `a` that no extent of sorted `b` covers.
fn minus(a: &[(u64, u64)], b: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let mut j = 0;
    for &(start, len) in a {
        let (mut at, end) = (start, start + len);
        while j < b.len() && b[j].0 + b[j].1 <= at {
            j += 1;
        }
        let mut k = j;
        while at < end {
            match b.get(k) {
                Some(&(s, l)) if s < end => {
                    if s > at {
                        out.push((at, s - at));
                    }
                    at = at.max(s + l);
                    k += 1;
                }
                _ => {
                    out.push((at, end - at));
                    at = end;
                }
            }
        }
    }
    out
}

impl<D: BlockDev> Fs<D> {
    /// Free blocks, and blocks in the data area.
    pub fn space(&self) -> (u64, u64) {
        let (start, end) = format::data_area(self.block_count());
        (self.io.alloc.free_blocks(), end - start)
    }

    /// Whether free space is under a tenth of the data area: when the GC must run,
    /// and retention drops the oldest undo snapshot.
    pub fn space_low(&self) -> bool {
        let (free, total) = self.space();
        free * 10 < total
    }

    pub fn gc_running(&self) -> bool {
        self.gc.is_some()
    }

    /// Starts a pass over the filesystem as the last commit left it.
    pub fn gc_start(&mut self) -> Result<(), Error> {
        self.writing()?;
        if self.gc.is_some() {
            return Err(Error::Invalid("a GC pass is already running"));
        }
        if !self.is_clean() {
            return Err(Error::Invalid("a GC pass starts between commits"));
        }
        // The volume table at the bottom of the stack, so it is read last: the
        // part of the table a pass reads latest is the part commits are likeliest
        // to have rewritten meanwhile.
        let table = self
            .volumes_root()
            .ok_or(Error::Invalid("the volume table is not written"))?;
        let mut stack = vec![(table, false)];
        let skip_snaps = self.io.faults.gc_skips_snapshots;
        for (id, _) in self.volumes() {
            let live = self
                .live_root_of(id)
                .ok_or(Error::Invalid("a GC pass starts between commits"))?;
            stack.push((live, true));
            let (_, snaps) = self.volume_state(id).ok_or(Error::NotFound)?;
            if !skip_snaps {
                stack.extend(snaps.values().map(|s| (s.root, true)));
            }
        }
        stack.retain(|(ptr, _)| !ptr.is_null());
        let marked: Vec<Run> = self.io.alloc.chain().iter().map(run_of).collect();
        let (start, end) = format::data_area(self.block_count());
        let used = minus(&[(start, end - start)], &self.io.alloc.unused());
        self.io.gc_pin = self.committed;
        self.gc = Some(Gc {
            generation: self.committed,
            stack,
            seen: BTreeSet::new(),
            marked,
            used,
            nodes: 0,
        });
        Ok(())
    }

    /// Reads up to `budget` nodes of the running pass. When none are left, frees
    /// every block that was in use when the pass began and that nothing reached,
    /// and ends the pass. A node that does not verify ends the pass with nothing
    /// freed.
    pub fn gc_step(&mut self, budget: u64) -> Result<GcProgress, Error> {
        let mut gc = self
            .gc
            .take()
            .ok_or(Error::Invalid("no GC pass is running"))?;
        for _ in 0..budget {
            let Some((ptr, values)) = gc.stack.pop() else {
                break;
            };
            if !gc.seen.insert(ptr.addr) {
                continue;
            }
            gc.marked.push(run_of(&ptr));
            gc.nodes += 1;
            let node = match self.io.load_node(&ptr) {
                Ok(node) => node,
                Err(e) => {
                    self.io.gc_pin = 0;
                    return Err(e);
                }
            };
            match node {
                Node::Leaf(items) if values => {
                    let records = items.iter().filter_map(|(_, raw)| record_of(raw));
                    gc.marked.extend(records.map(|(record, _)| run_of(&record)));
                }
                Node::Leaf(_) => {}
                Node::Branch(_, children) => {
                    for (_, child) in children {
                        if let Child::Clean(child) = child {
                            gc.stack.push((child, values));
                        }
                    }
                }
            }
        }
        if !gc.stack.is_empty() {
            let progress = GcProgress {
                done: false,
                nodes: gc.nodes,
                freed_blocks: 0,
            };
            self.gc = Some(gc);
            return Ok(progress);
        }
        self.io.gc_pin = 0;
        let reached = extents(core::mem::take(&mut gc.marked));
        let leaked = minus(&minus(&gc.used, &reached), &self.io.alloc.unused());
        let txn = self.io.txn;
        let mut freed = 0;
        for (start, len) in leaked {
            for run in tile(start, start + len) {
                freed += run.blocks();
                self.io.alloc.free_later(txn, run);
            }
        }
        Ok(GcProgress {
            done: true,
            nodes: gc.nodes,
            freed_blocks: freed,
        })
    }

    /// A whole pass, in slices of `budget` nodes. Returns the blocks it freed.
    pub fn gc(&mut self, budget: u64) -> Result<u64, Error> {
        self.gc_start()?;
        loop {
            let progress = self.gc_step(budget.max(1))?;
            if progress.done {
                return Ok(progress.freed_blocks);
            }
        }
    }

    /// Drops every undo snapshot whose expiry is at or before `now`, and, if
    /// free space is low, the oldest undo snapshot left, with their UNDO records.
    pub fn expire(&mut self, now: u64) -> Result<Vec<Dropped>, Error> {
        self.writing()?;
        let active = self.volume();
        let result = self.expire_all(now);
        self.set_volume(active)?;
        result
    }

    fn expire_all(&mut self, now: u64) -> Result<Vec<Dropped>, Error> {
        let mut out = Vec::new();
        let mut oldest: Option<(u64, u64, u64, u64)> = None;
        for (volume, _) in self.volumes() {
            self.set_volume(volume)?;
            let undo: Vec<(u64, u64, u64, u64)> = self
                .snapshots()
                .iter()
                .filter(|(_, s)| s.kind == SnapKind::Undo)
                .map(|(id, s)| (*id, s.generation, s.expiry, s.pin))
                .collect();
            for (snapshot, generation, expiry, action) in undo {
                if expiry != 0 && expiry <= now {
                    self.drop_snapshot(snapshot)?;
                    out.push(Dropped {
                        volume,
                        snapshot,
                        action,
                        why: Why::Expired,
                    });
                } else if oldest.is_none_or(|(_, _, g, _)| generation < g) {
                    oldest = Some((volume, snapshot, generation, action));
                }
            }
        }
        if let Some((volume, snapshot, _, action)) = oldest.filter(|_| self.space_low()) {
            self.set_volume(volume)?;
            self.drop_snapshot(snapshot)?;
            out.push(Dropped {
                volume,
                snapshot,
                action,
                why: Why::Space,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extents_join_and_subtract() {
        let runs = vec![
            Run { addr: 8, level: 1 },
            Run { addr: 10, level: 0 },
            Run { addr: 16, level: 2 },
        ];
        assert_eq!(extents(runs), vec![(8, 3), (16, 4)]);
        assert_eq!(
            minus(&[(0, 32)], &[(8, 3), (16, 4)]),
            vec![(0, 8), (11, 5), (20, 12)]
        );
        assert_eq!(minus(&[(0, 4), (10, 4)], &[(2, 10)]), vec![(0, 2), (12, 2)]);
        assert_eq!(minus(&[(5, 5)], &[]), vec![(5, 5)]);
        assert_eq!(minus(&[(5, 5)], &[(0, 20)]), vec![]);
    }
}
