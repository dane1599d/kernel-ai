//! What `check --repair` holds a volume's trees to on births and blocks (rules
//! 1, 2 and 7). A writer frees a replaced block on its pointer's birth's word,
//! and the allocator's chain and the volume table free what they replace at a
//! commit, so `check` refuses one block reached with two births, whichever
//! structures the two pointers lie in.
//!
//! - **Births.** A block a volume's trees reach takes the birth most of them
//!   give it. A snapshot's word counts twice and the live tree's once: the live
//!   tree is rebuilt from its items and loses nothing, while a snapshot that
//!   disagrees is dropped. On a tie the lower birth wins, since a block whose
//!   birth is too low only waits for the GC. A lying snapshot is outvoted by an
//!   honest one only if a third tree corroborates it; on its own, which of the
//!   two lies cannot be known.
//! - **Others.** A block the allocator's chain, the volume table or a volume
//!   with a lower id reaches is theirs. No writer shares a block across
//!   volumes, and the chain and the table cannot be dropped. A snapshot that
//!   reaches one is dropped; a live tree that does is rebuilt, its records that
//!   do copied.

use crate::allocator::run_of;
use crate::buddy::Run;
use crate::check::{Births, Cache, Held, tree_runs};
use crate::dev::BlockDev;
use crate::format::BlockPtr;
use crate::fs::Fs;
use crate::repair_fs::reach;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// The birth each run takes, from the run lists of a volume's snapshots and,
/// if it can vote, of its live tree.
pub fn chosen(snaps: &[&[Held]], live: Option<&[Held]>) -> Births {
    let mut votes: BTreeMap<Run, BTreeMap<u64, u32>> = BTreeMap::new();
    let weighted = snaps.iter().map(|l| (*l, 2)).chain(live.map(|l| (l, 1)));
    for (list, weight) in weighted {
        for h in list {
            *votes.entry(h.run).or_default().entry(h.birth).or_default() += weight;
        }
    }
    let held: Vec<Held> = votes
        .into_iter()
        .filter_map(|(run, by)| {
            // Births in order, so a tie keeps the lowest.
            let best = by.into_iter().fold(None, |best, (birth, n)| match best {
                Some((_, most)) if most >= n => best,
                _ => Some((birth, n)),
            });
            best.map(|(birth, _)| Held { run, birth })
        })
        .collect();
    let mut births = Births::default();
    births.add(&held);
    births
}

/// The live tree `root`'s runs, if they can vote: its root no newer than the
/// commit, its tree read whole, and no block another structure's.
pub fn live_runs<D: BlockDev>(
    fs: &mut Fs<D>,
    root: &BlockPtr,
    others: &Others,
    cache: &mut Cache,
) -> Result<Vec<Held>, String> {
    if root.birth > fs.committed {
        return Err(String::from("its root is newer than its commit"));
    }
    let runs = tree_runs(&mut fs.io, root, cache)?;
    match others.clash(&runs) {
        Some(why) => Err(why),
        None => Ok(runs),
    }
}

/// Blocks other structures reach, as disjoint spans, each with whose it is.
#[derive(Default)]
pub struct Others(BTreeMap<u64, (u64, String)>);

impl Others {
    /// The allocator's chain.
    pub fn chain<D: BlockDev>(fs: &Fs<D>) -> Others {
        let mut others = Others::default();
        let runs: Vec<Run> = fs.io.alloc.chain().iter().map(run_of).collect();
        others.add(&runs, "in the allocator's chain");
        others
    }

    /// The allocator's chain, the volume table and every volume with an id
    /// below `volume`, as far as they read.
    pub fn before<D: BlockDev>(fs: &mut Fs<D>, volume: u64) -> Others {
        let mut others = Self::chain(fs);
        let mut runs = Vec::new();
        if let Some(root) = fs.volumes_root() {
            reach(&mut fs.io, &root, false, &mut runs, &mut BTreeSet::new());
        }
        others.add(&runs, "in the volume table");
        for (id, _) in fs.volumes().into_iter().filter(|(id, _)| *id < volume) {
            let mut roots: Vec<BlockPtr> = fs.live_root_of(id).into_iter().collect();
            if let Some((_, snaps)) = fs.volume_state(id) {
                roots.extend(snaps.values().map(|s| s.root));
            }
            let (mut runs, mut seen) = (Vec::new(), BTreeSet::new());
            for root in roots {
                reach(&mut fs.io, &root, true, &mut runs, &mut seen);
            }
            others.add(&runs, &format!("in volume {id}"));
        }
        others
    }

    fn add(&mut self, runs: &[Run], whose: &str) {
        for run in runs {
            let (mut start, mut end, mut name) = (run.addr, run.end(), String::from(whose));
            // Spans this run touches are merged into one, under an earlier name.
            loop {
                let found = self.0.range(..end).next_back().map(|(&a, (e, _))| (a, *e));
                let Some((a, e)) = found.filter(|&(_, e)| e > start) else {
                    break;
                };
                if let Some((_, was)) = self.0.remove(&a) {
                    name = was;
                }
                (start, end) = (start.min(a), end.max(e));
            }
            self.0.insert(start, (end, name));
        }
    }

    /// Whose a block of `run` is, if another structure's.
    pub fn whose(&self, run: &Run) -> Option<&str> {
        let (_, (end, whose)) = self.0.range(..run.end()).next_back()?;
        (*end > run.addr).then_some(whose.as_str())
    }

    /// Why `runs` cannot be a tree of this volume: a block another structure
    /// reaches.
    pub fn clash(&self, runs: &[Held]) -> Option<String> {
        runs.iter().find_map(|h| {
            let whose = self.whose(&h.run)?;
            Some(format!("block {} is also {whose}", h.run.addr))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(addr: u64, birth: u64) -> Held {
        Held {
            run: Run { addr, level: 0 },
            birth,
        }
    }

    #[test]
    fn a_birth_two_trees_give_wins_and_a_tie_takes_the_lowest() {
        let (lie, honest) = ([held(10, 9)], [held(10, 4)]);
        // Two snapshots alone: a tie.
        assert_eq!(chosen(&[&lie, &honest], None).of(&lie[0].run), Some(4));
        let lie = [held(10, 2)];
        assert_eq!(chosen(&[&lie, &honest], None).of(&lie[0].run), Some(2));
        // The live tree corroborates one.
        let live = [held(10, 4)];
        assert_eq!(
            chosen(&[&lie, &honest], Some(&live)).of(&lie[0].run),
            Some(4)
        );
        // A snapshot outweighs the live tree.
        let lie = [held(10, 1)];
        assert_eq!(chosen(&[&honest], Some(&lie)).of(&lie[0].run), Some(4));
    }

    #[test]
    fn spans_that_touch_merge_and_a_run_inside_one_is_found() {
        let mut others = Others::default();
        let run = |addr, level| Run { addr, level };
        others.add(&[run(8, 2), run(20, 0)], "a");
        others.add(&[run(10, 2)], "b");
        assert_eq!(others.whose(&run(13, 0)), Some("a"));
        assert_eq!(others.whose(&run(12, 0)), Some("a"));
        assert_eq!(others.whose(&run(14, 0)), None);
        assert_eq!(others.whose(&run(16, 3)), Some("a"));
        assert_eq!(others.whose(&run(0, 3)), None);
        assert_eq!(others.whose(&run(0, 4)), Some("a"));
        assert_eq!(others.whose(&run(21, 0)), None);
        assert_eq!(others.0.len(), 2);
    }
}
