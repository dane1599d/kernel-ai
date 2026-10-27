//! `check()`: everything a mounted filesystem reaches verifies and is accounted
//! for; and the memo that lets the crash gates check a million states soundly.
//!
//! **check()** reads every node and record reachable from the live tree, each
//! snapshot and the volume table, and the allocator's chain. Each must verify;
//! keys lie in order and inside their parents' bounds; levels step down; every
//! node off a tree's rightmost path is at least a quarter full; record lengths
//! are sound. No
//! block may be both reached and free or pending, two different runs may not
//! overlap, and all of it lies in the data area. It reports each tree's digest and
//! item count, and the blocks reached, free, pending and leaked (held by nothing,
//! waiting for the GC).
//!
//! **The memo.** A subtree is summarised once, by its root pointer's address and
//! checksum. A summary is trusted only while no block it covers has been written
//! since: [`Cache::forget`] drops every summary covering a block a crash epoch
//! made durable, and [`Cache::taint`] hides, for one crash state, every summary
//! covering a block that state's open epoch wrote. Summaries are found through the
//! blocks they hold directly (their node, their leaf's records, an allocator
//! block) and the links to the summaries above them, so a block overwritten too
//! early is read again and caught. Keying by address and checksum alone, as spike
//! S04-1 did, would have trusted it. A whole mount is memoised the same way, by its
//! allocator head, so most crash states cost a ring read and a lookup.

use crate::allocator::run_of;
use crate::buddy::Run;
use crate::csum::Csum;
use crate::dev::BlockDev;
use crate::format::{self, BlockPtr};
use crate::fs::{Fs, Io, Value, decode_value, record_of};
use crate::tree::{Child, Key, Node};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// What a subtree holds.
#[derive(Clone, Debug)]
pub struct Summary {
    pub count: u64,
    /// The sum of every item's hash, records read in full: order does not matter.
    pub digest: u64,
    pub min: Key,
    pub max: Key,
    /// The node, or a node on its rightmost path down, is under a quarter full:
    /// allowed only on the tree's own rightmost path, where appends leave them.
    pub spine_underfull: bool,
    /// Every run the subtree occupies, its nodes and records, by address.
    pub runs: Rc<Vec<Run>>,
}

/// What `check()` found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub generation: u64,
    /// The live tree's digest and item count.
    pub live: (u64, u64),
    pub snaps: BTreeMap<u64, (u64, u64)>,
    pub reachable_blocks: u64,
    pub free_blocks: u64,
    pub pending_blocks: u64,
    /// Blocks neither reached, free nor pending: the GC's to reclaim.
    pub leaked_blocks: u64,
}

/// A memo key: a subtree's root pointer (address, checksum), or a mount's
/// allocator head, told apart by the last field.
type MemoKey = (u64, Csum, u8);
const TREE: u8 = 0;
const MOUNT: u8 = 1;
const META: u8 = 2;

/// What a tree's leaves hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leaves {
    /// A volume's values: inline bytes or records.
    Values,
    /// The volume table's entries, read as bytes.
    Entries,
}

impl Leaves {
    fn tag(self) -> u8 {
        match self {
            Leaves::Values => TREE,
            Leaves::Entries => META,
        }
    }
}

enum Memo {
    Tree(Rc<Summary>),
    Mount(Rc<Report>, BlockPtr),
}

struct Entry {
    memo: Memo,
    parents: Vec<MemoKey>,
}

#[derive(Default)]
pub struct Cache {
    entries: BTreeMap<MemoKey, Entry>,
    /// A block, and the memos that hold it directly.
    holders: BTreeMap<u64, Vec<MemoKey>>,
    tainted: BTreeSet<MemoKey>,
    overlay: BTreeSet<u64>,
    pub hits: u64,
    pub misses: u64,
}

impl Cache {
    /// `start` and every memo above them.
    fn above(&self, start: Vec<MemoKey>) -> BTreeSet<MemoKey> {
        let mut out = BTreeSet::new();
        let mut stack = start;
        while let Some(key) = stack.pop() {
            if out.insert(key)
                && let Some(entry) = self.entries.get(&key)
            {
                stack.extend(entry.parents.iter().copied());
            }
        }
        out
    }

    fn holding(&self, blocks: impl IntoIterator<Item = u64>) -> Vec<MemoKey> {
        blocks
            .into_iter()
            .filter_map(|b| self.holders.get(&b))
            .flatten()
            .copied()
            .collect()
    }

    /// `written` are durable now: forget every memo that covers one of them.
    pub fn forget(&mut self, written: &[u64]) {
        for key in self.above(self.holding(written.iter().copied())) {
            self.entries.remove(&key);
        }
        for block in written {
            self.holders.remove(block);
        }
    }

    /// The next state wrote `overlay` over the durable disk: hide every memo that
    /// covers one of those blocks, for that state only.
    pub fn taint(&mut self, overlay: impl IntoIterator<Item = u64>) {
        self.overlay = overlay.into_iter().collect();
        self.tainted = self.above(self.holding(self.overlay.iter().copied()));
    }

    fn usable(&mut self, key: &MemoKey) -> Option<&Memo> {
        if self.tainted.contains(key) {
            return None;
        }
        let found = self.entries.get(key).map(|e| &e.memo);
        match found {
            Some(_) => self.hits += 1,
            None => self.misses += 1,
        }
        self.entries.get(key).map(|e| &e.memo)
    }

    /// The report of a mount of `sb` already checked, if one is remembered and
    /// still holds: what the oracle asks before it mounts at all.
    pub fn mounted(&mut self, sb: &format::Superblock) -> Option<Report> {
        let key = (sb.alloc.addr, sb.alloc.csum, MOUNT);
        match self.usable(&key) {
            Some(Memo::Mount(report, volumes))
                if *volumes == sb.volumes && report.generation == sb.generation =>
            {
                Some((**report).clone())
            }
            _ => None,
        }
    }

    /// Remembers `memo` under `key`, unless it read a block this state wrote or
    /// rests on a memo not remembered: such a memo would not be forgotten when it
    /// should be.
    fn put(&mut self, key: MemoKey, memo: Memo, own: &[u64], children: &[MemoKey]) {
        let rests_on_unknown = children
            .iter()
            .any(|c| !self.entries.contains_key(c) || self.tainted.contains(c));
        if rests_on_unknown || own.iter().any(|b| self.overlay.contains(b)) {
            return;
        }
        for child in children {
            if let Some(entry) = self.entries.get_mut(child) {
                entry.parents.push(key);
            }
        }
        for block in own {
            self.holders.entry(*block).or_default().push(key);
        }
        self.entries.insert(
            key,
            Entry {
                memo,
                parents: Vec::new(),
            },
        );
    }
}

/// An item's hash, the record's bytes for a record.
pub fn item_hash(key: &Key, value: &Value) -> u64 {
    let (tag, bytes) = match value {
        Value::Inline(bytes) => (0u8, bytes),
        Value::Record(bytes) => (1u8, bytes),
    };
    let mut buf = Vec::with_capacity(18 + bytes.len());
    buf.extend_from_slice(&key.oid.to_le_bytes());
    buf.push(key.kind);
    buf.extend_from_slice(&key.off.to_le_bytes());
    buf.push(tag);
    buf.extend_from_slice(bytes);
    seahash::hash(&buf)
}

/// The digest and count of a model's map, to compare with a tree's.
pub fn map_digest<'a>(items: impl IntoIterator<Item = (&'a Key, &'a Value)>) -> (u64, u64) {
    items
        .into_iter()
        .fold((0, 0), |(digest, count), (key, value)| {
            (digest.wrapping_add(item_hash(key, value)), count + 1)
        })
}

/// Joins sorted run lists, keeping one copy of a run several trees share, and
/// refusing two different runs that overlap.
fn merge(lists: &[&[Run]]) -> Result<Vec<Run>, String> {
    let mut all: Vec<Run> = lists.iter().flat_map(|l| l.iter().copied()).collect();
    all.sort_unstable();
    all.dedup();
    if let Some(w) = all.windows(2).find(|w| w[0].end() > w[1].addr) {
        return Err(format!(
            "blocks used twice: runs at {} and {}",
            w[0].addr, w[1].addr
        ));
    }
    Ok(all)
}

/// Summarises the subtree `ptr` names.
pub fn summarise<D: BlockDev>(
    io: &mut Io<D>,
    ptr: &BlockPtr,
    leaves: Leaves,
    cache: &mut Cache,
) -> Result<Rc<Summary>, String> {
    let key = (ptr.addr, ptr.csum, leaves.tag());
    if let Some(Memo::Tree(summary)) = cache.usable(&key) {
        return Ok(summary.clone());
    }
    let node = io
        .load_node(ptr)
        .map_err(|e| format!("node {}: {e:?}", ptr.addr))?;
    let mut spine_underfull = node.underfull();
    let mut own = alloc::vec![ptr.addr];
    let mut lists: Vec<Rc<Vec<Run>>> = Vec::new();
    let mut runs = alloc::vec![run_of(ptr)];
    let mut children: Vec<MemoKey> = Vec::new();
    let (mut count, mut digest) = (0u64, 0u64);
    let (min, max) = match node {
        Node::Leaf(items) => {
            let (Some((min, _)), Some((max, _))) = (items.first(), items.last()) else {
                return Err(format!("empty leaf {}", ptr.addr));
            };
            let (min, max) = (*min, *max);
            for (key, raw) in &items {
                let value = match leaves {
                    Leaves::Entries => Value::Inline(raw.clone()),
                    Leaves::Values => {
                        if let Some((record, _)) = record_of(raw) {
                            runs.push(run_of(&record));
                            own.extend(record.addr..record.addr + u64::from(record.blocks));
                        }
                        decode_value(raw, io).map_err(|e| format!("leaf {}: {e:?}", ptr.addr))?
                    }
                };
                digest = digest.wrapping_add(item_hash(key, &value));
                count += 1;
            }
            (min, max)
        }
        Node::Branch(level, entries) => {
            let mut bounds: Vec<(Key, Key)> = Vec::with_capacity(entries.len());
            for (i, (lower, child)) in entries.iter().enumerate() {
                let Child::Clean(child_ptr) = child else {
                    unreachable!("a node read from disk is clean");
                };
                if child_ptr.level + 1 != level {
                    return Err(format!("branch {}: a child at the wrong level", ptr.addr));
                }
                let sub = summarise(io, child_ptr, leaves, cache)?;
                let upper = entries.get(i + 1).map(|(k, _)| *k);
                if sub.min < *lower || upper.is_some_and(|u| sub.max >= u) {
                    return Err(format!(
                        "branch {}: child keys outside their bounds",
                        ptr.addr
                    ));
                }
                let last = i + 1 == entries.len();
                if sub.spine_underfull && !last {
                    return Err(format!("branch {}: a child under a quarter full", ptr.addr));
                }
                spine_underfull |= last && sub.spine_underfull;
                bounds.push((sub.min, sub.max));
                count += sub.count;
                digest = digest.wrapping_add(sub.digest);
                lists.push(sub.runs.clone());
                children.push((child_ptr.addr, child_ptr.csum, leaves.tag()));
            }
            (bounds[0].0, bounds[bounds.len() - 1].1)
        }
    };
    let mut parts: Vec<&[Run]> = lists.iter().map(|l| &l[..]).collect();
    parts.push(&runs);
    let total: usize = parts.iter().map(|p| p.len()).sum();
    let merged = merge(&parts)?;
    if merged.len() != total {
        return Err(format!("subtree {}: a block reached twice", ptr.addr));
    }
    let summary = Rc::new(Summary {
        count,
        digest,
        min,
        max,
        spine_underfull,
        runs: Rc::new(merged),
    });
    cache.put(key, Memo::Tree(summary.clone()), &own, &children);
    Ok(summary)
}

/// `check()` on a filesystem as its last commit left it.
pub fn check<D: BlockDev>(fs: &mut Fs<D>, cache: &mut Cache) -> Result<Report, String> {
    if !fs.is_clean() {
        return Err("check() runs between commits".to_string());
    }
    let head = fs.io.alloc.head();
    let mount_key = (head.addr, head.csum, MOUNT);
    let volumes = fs.volumes_root().ok_or("the volume table is not written")?;
    if let Some(Memo::Mount(report, vol)) = cache.usable(&mount_key)
        && *vol == volumes
        && report.generation == fs.committed
    {
        return Ok((**report).clone());
    }
    let live = fs.live_root().ok_or("the live tree is not written")?;
    let summary = |fs: &mut Fs<D>, cache: &mut Cache, ptr: &BlockPtr, leaves| match ptr.is_null() {
        true => Ok(None),
        false => summarise(&mut fs.io, ptr, leaves, cache).map(Some),
    };
    let vol = summary(fs, cache, &volumes, Leaves::Entries)?.ok_or("an empty volume table")?;
    let live_summary = summary(fs, cache, &live, Leaves::Values)?;
    let roots: Vec<(u64, BlockPtr)> = fs.snapshots().iter().map(|(id, s)| (*id, s.root)).collect();
    let mut snaps = BTreeMap::new();
    let mut lists: Vec<Rc<Vec<Run>>> = alloc::vec![vol.runs.clone()];
    let mut children = alloc::vec![(volumes.addr, volumes.csum, META)];
    for (id, root) in roots {
        let sub = summary(fs, cache, &root, Leaves::Values)?;
        snaps.insert(id, sub.as_ref().map_or((0, 0), |s| (s.digest, s.count)));
        if let Some(s) = sub {
            lists.push(s.runs.clone());
            children.push((root.addr, root.csum, TREE));
        }
    }
    if let Some(s) = &live_summary {
        lists.push(s.runs.clone());
        children.push((live.addr, live.csum, TREE));
    }
    let chain: Vec<Run> = fs.io.alloc.chain().iter().map(run_of).collect();
    let mut parts: Vec<&[Run]> = lists.iter().map(|l| &l[..]).collect();
    parts.push(&chain);
    let reached = merge(&parts)?;
    let (start, end) = format::data_area(fs.block_count());
    if reached.first().is_some_and(|r| r.addr < start)
        || reached.last().is_some_and(|r| r.end() > end)
    {
        return Err("a block outside the data area".to_string());
    }
    let unused = fs.io.alloc.unused();
    let (mut i, mut j) = (0, 0);
    while i < reached.len() && j < unused.len() {
        let (run, (s, l)) = (reached[i], unused[j]);
        if run.addr < s + l && s < run.end() {
            return Err(format!(
                "run at {} is reached and also free or pending",
                run.addr
            ));
        }
        if run.end() <= s {
            i += 1;
        } else {
            j += 1;
        }
    }
    let state = fs.io.alloc.state();
    let reachable_blocks: u64 = reached.iter().map(|r| r.blocks()).sum();
    let free_blocks: u64 = state.free.iter().map(|(_, l)| l).sum();
    let pending_blocks: u64 = state.pending.iter().map(|(_, _, l)| l).sum();
    let report = Report {
        generation: fs.committed,
        live: live_summary.map_or((0, 0), |s| (s.digest, s.count)),
        snaps,
        reachable_blocks,
        free_blocks,
        pending_blocks,
        leaked_blocks: (end - start)
            .saturating_sub(reachable_blocks + free_blocks + pending_blocks),
    };
    let own: Vec<u64> = fs.io.alloc.chain().iter().map(|p| p.addr).collect();
    cache.put(
        mount_key,
        Memo::Mount(Rc::new(report.clone()), volumes),
        &own,
        &children,
    );
    Ok(report)
}
