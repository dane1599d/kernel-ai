//! `check()`, the GC's mark, and state digests.
//!
//! Every subtree is summarised by its root pointer's `(addr, csum)`. Because the
//! checksum sits in the parent, the same pair means the same bytes on any disk
//! image, so summaries are memoised across the crash harness's images: a crash
//! state costs only the nodes its writes changed.

use crate::alloc_buddy::{LEVELS, Run};
use crate::disk::BlockDev;
use crate::format::{self, BlockPtr};
use crate::fs::{Error, Fs, Io, Value, decode_value, run_of};
use crate::tree::{Child, Node, Store};
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// What a subtree holds.
#[derive(Clone, Debug)]
pub struct Summary {
    pub min_key: u64,
    pub max_key: u64,
    pub count: u64,
    /// Order-independent digest of every (key, value), values read through records.
    pub digest: u64,
    /// Every run the subtree occupies: its nodes and records.
    pub runs: Rc<Vec<Run>>,
}

/// Memoised summaries, by `(addr, csum, deep)`.
#[derive(Default)]
pub struct Cache {
    map: BTreeMap<(u64, [u8; 32], bool), Rc<Summary>>,
}

impl Cache {
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

fn item_digest(key: u64, value: &Value) -> u64 {
    let (tag, bytes) = match value {
        Value::Inline(bytes) => (0u8, bytes),
        Value::Record(bytes) => (1u8, bytes),
    };
    let mut buf = Vec::with_capacity(9 + bytes.len());
    buf.extend_from_slice(&key.to_le_bytes());
    buf.push(tag);
    buf.extend_from_slice(bytes);
    seahash::hash(&buf)
}

/// The digest of a model's map, to compare with a tree's.
pub fn map_digest<'a>(items: impl IntoIterator<Item = (&'a u64, &'a Value)>) -> (u64, u64) {
    items
        .into_iter()
        .fold((0, 0), |(digest, count), (key, value)| {
            (digest.wrapping_add(item_digest(*key, value)), count + 1)
        })
}

/// Summarises the subtree `ptr`; `deep` also reads every record and digests the
/// values (the GC's mark does neither).
pub fn summarise<D: BlockDev>(
    io: &mut Io<D>,
    ptr: &BlockPtr,
    deep: bool,
    cache: &mut Cache,
) -> Result<Rc<Summary>, Error> {
    let memo = (ptr.addr, ptr.csum, deep);
    if let Some(summary) = cache.map.get(&memo) {
        return Ok(summary.clone());
    }
    let node = io.load(ptr)?;
    let mut runs = alloc::vec![run_of(ptr)];
    let summary = match node {
        Node::Leaf(items) => {
            let mut digest = 0u64;
            for pair in items.windows(2) {
                if pair[0].0 >= pair[1].0 {
                    return Err(Error::Corrupt("leaf keys out of order"));
                }
            }
            for (key, raw) in &items {
                if let Some((record, _)) = crate::fs::record_of(raw) {
                    runs.push(run_of(&record));
                }
                if deep {
                    let value = decode_value(raw, io)?;
                    digest = digest.wrapping_add(item_digest(*key, &value));
                } else if raw.is_empty() {
                    return Err(Error::Corrupt("empty value"));
                }
            }
            Summary {
                min_key: items.first().map_or(0, |i| i.0),
                max_key: items.last().map_or(0, |i| i.0),
                count: items.len() as u64,
                digest,
                runs: Rc::new(runs),
            }
        }
        Node::Branch(level, children) => {
            let (mut count, mut digest) = (0u64, 0u64);
            let mut bounds = Vec::with_capacity(children.len());
            for (i, (lower, child)) in children.iter().enumerate() {
                let Child::Clean(child_ptr) = child else {
                    unreachable!("a node read from disk is clean");
                };
                if child_ptr.level + 1 != level {
                    return Err(Error::Corrupt("child level"));
                }
                let sub = summarise(io, child_ptr, deep, cache)?;
                let upper = children.get(i + 1).map(|(k, _)| *k);
                if sub.count == 0 || sub.min_key < *lower || upper.is_some_and(|u| sub.max_key >= u)
                {
                    return Err(Error::Corrupt("child keys outside their bounds"));
                }
                bounds.push((sub.min_key, sub.max_key));
                count += sub.count;
                digest = digest.wrapping_add(sub.digest);
                runs.extend_from_slice(&sub.runs);
            }
            Summary {
                min_key: bounds.first().map_or(0, |b| b.0),
                max_key: bounds.last().map_or(0, |b| b.1),
                count,
                digest,
                runs: Rc::new(runs),
            }
        }
    };
    let summary = Rc::new(summary);
    cache.map.insert(memo, summary.clone());
    Ok(summary)
}

/// What `check()` found on a clean filesystem.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub live: (u64, u64),
    pub snaps: BTreeMap<u64, (u64, u64)>,
    pub reachable_blocks: u64,
    pub free_blocks: u64,
    pub pending_blocks: u64,
    /// Blocks neither reachable, free nor pending: the GC's to reclaim.
    pub leaked_blocks: u64,
}

/// Every run the committed state reaches, sorted and deduplicated, and the
/// summaries of the live tree and each snapshot.
type Reached = (
    Vec<Run>,
    Option<Rc<Summary>>,
    BTreeMap<u64, Option<Rc<Summary>>>,
);

fn reach<D: BlockDev>(fs: &mut Fs<D>, deep: bool, cache: &mut Cache) -> Result<Reached, String> {
    let err = |e: Error| alloc::format!("{e:?}");
    let mut runs = Vec::new();
    let live = match fs.live_root() {
        Some(ptr) => Some(summarise(&mut fs.io, &ptr, deep, cache).map_err(err)?),
        None => None,
    };
    let mut snaps = BTreeMap::new();
    let roots: Vec<(u64, Option<BlockPtr>)> =
        fs.snaps.iter().map(|(id, s)| (*id, s.root)).collect();
    for (id, root) in roots {
        let summary = match root {
            Some(ptr) => Some(summarise(&mut fs.io, &ptr, deep, cache).map_err(err)?),
            None => None,
        };
        snaps.insert(id, summary);
    }
    let volumes = fs.volumes_root().ok_or("the volume table is not clean")?;
    let vol = summarise(&mut fs.io, &volumes, false, cache).map_err(err)?;
    runs.extend_from_slice(&vol.runs);
    for summary in live.iter().chain(snaps.values().flatten()) {
        runs.extend_from_slice(&summary.runs);
    }
    runs.extend(fs.alloc_chain().iter().map(run_of));
    runs.sort_unstable();
    runs.dedup();
    for pair in runs.windows(2) {
        if pair[0].end() > pair[1].addr {
            return Err(alloc::format!(
                "blocks used twice: {:?} and {:?}",
                pair[0],
                pair[1]
            ));
        }
    }
    Ok((runs, live, snaps))
}

fn overlaps(sorted: &[Run], run: Run) -> bool {
    let i = sorted.partition_point(|r| r.end() <= run.addr);
    sorted.get(i).is_some_and(|r| r.addr < run.end())
}

/// Checks a mounted filesystem: every checksum and node on every path, record
/// lengths, and that no block is both reached and free or pending.
pub fn check<D: BlockDev>(fs: &mut Fs<D>, cache: &mut Cache) -> Result<Report, String> {
    let (runs, live, snaps) = reach(fs, true, cache)?;
    let (start, end) = format::data_area(fs.block_count());
    if runs.first().is_some_and(|r| r.addr < start) || runs.last().is_some_and(|r| r.end() > end) {
        return Err("a block outside the data area".to_string());
    }
    let mut report = Report {
        reachable_blocks: runs.iter().map(|r| r.blocks()).sum(),
        ..Report::default()
    };
    let mut other: Vec<Run> = Vec::new();
    for run in fs.io.alloc.free_runs() {
        if run.level as usize >= LEVELS || overlaps(&runs, run) {
            return Err(alloc::format!("free run {run:?} is in use"));
        }
        report.free_blocks += run.blocks();
        other.push(run);
    }
    for (_, run) in &fs.io.alloc.pending {
        if overlaps(&runs, *run) {
            return Err(alloc::format!("pending run {run:?} is in use"));
        }
        report.pending_blocks += run.blocks();
        other.push(*run);
    }
    other.sort_unstable();
    for pair in other.windows(2) {
        if pair[0].end() > pair[1].addr {
            return Err(alloc::format!(
                "free runs overlap: {:?} and {:?}",
                pair[0],
                pair[1]
            ));
        }
    }
    let used = report.reachable_blocks + report.free_blocks + report.pending_blocks;
    report.leaked_blocks = (end - start).saturating_sub(used);
    report.live = live.map_or((0, 0), |s| (s.digest, s.count));
    report.snaps = snaps
        .into_iter()
        .map(|(id, s)| (id, s.map_or((0, 0), |s| (s.digest, s.count))))
        .collect();
    Ok(report)
}

/// Mark-and-sweep (HAMMER2's method, no reference counts): every block the live
/// tree, the snapshots, the volume table and the allocator chain do not reach, and
/// that is neither free nor pending, is freed in the current transaction, so it
/// is reusable two commits later. Call it between commits. Returns the blocks
/// reclaimed.
pub fn gc<D: BlockDev>(fs: &mut Fs<D>) -> Result<u64, String> {
    if !fs.is_clean() {
        return Err("the GC runs between commits".to_string());
    }
    let mut cache = Cache::default();
    let (mut used, _, _) = reach(fs, false, &mut cache)?;
    used.extend(fs.io.alloc.free_runs());
    used.extend(fs.io.alloc.pending.iter().map(|(_, run)| *run));
    used.sort_unstable();
    let (start, end) = format::data_area(fs.block_count());
    let mut reclaimed = 0;
    let mut cursor = start;
    let txn = fs.io.txn;
    for run in used.iter().copied().chain(core::iter::once(Run {
        addr: end,
        level: 0,
    })) {
        let mut addr = cursor;
        while addr < run.addr.min(end) {
            let mut level = (LEVELS - 1) as u8;
            while level > 0 && (!addr.is_multiple_of(1 << level) || addr + (1 << level) > run.addr)
            {
                level -= 1;
            }
            fs.io.alloc.free_later(txn, Run { addr, level });
            reclaimed += 1 << level;
            addr += 1 << level;
        }
        cursor = cursor.max(run.end());
    }
    Ok(reclaimed)
}
