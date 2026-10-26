//! The crash harness (S04-1): runs a workload on a `RecordingDisk` beside a model,
//! then rebuilds every crash state the ADR names from the log and checks the
//! oracle on each.
//!
//! **Crash states.** The log splits into flush epochs. A crash leaves every write
//! before the last completed flush, plus any subset of the open epoch's writes:
//! - every prefix of the log;
//! - every subset of an epoch of up to 12 writes, and 4,096 random subsets above
//!   that;
//! - every superblock-slot write torn at each 512-byte step, with every subset (or
//!   64 random ones) of the epoch's other writes.
//!
//! **Oracle.** The image mounts, `check()` is clean, and its state equals one
//! committed generation no older than the last commit that was waited on (whose
//! second flush completed before the crash).

use crate::check::{self, Cache, map_digest};
use crate::disk::{Image, MemDisk, Op, RecordingDisk};
use crate::format;
use crate::{Block, CsumKind, Fs, Value};
use alloc::rc::Rc;
use std::collections::{BTreeMap, BTreeSet};
use std::string::String;
use std::vec::Vec;

/// A state as the oracle compares it: the live tree's digest and count, and each
/// snapshot's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub live: (u64, u64),
    pub snaps: BTreeMap<u64, (u64, u64)>,
}

/// What the workload means, kept beside the filesystem.
#[derive(Clone, Debug, Default)]
pub struct Model {
    pub live: BTreeMap<u64, Value>,
    pub snaps: BTreeMap<u64, BTreeMap<u64, Value>>,
    pub next_snap: u64,
}

impl Model {
    pub fn new() -> Self {
        Self {
            next_snap: 1,
            ..Self::default()
        }
    }

    pub fn state(&self) -> State {
        State {
            live: map_digest(&self.live),
            snaps: self
                .snaps
                .iter()
                .map(|(id, map)| (*id, map_digest(map)))
                .collect(),
        }
    }
}

/// One workload operation (ADR 0004 S04-1's set).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Put,
    Delete,
    Overwrite,
    Snapshot,
    DropSnapshot,
    Restore,
    Commit,
}

pub const ACTIONS: [Action; 7] = [
    Action::Put,
    Action::Delete,
    Action::Overwrite,
    Action::Snapshot,
    Action::DropSnapshot,
    Action::Restore,
    Action::Commit,
];

/// Marsaglia's xorshift64, for repeatable random choices.
#[derive(Clone)]
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0 | 1;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }
}

/// A filesystem and its model, driven together.
pub struct Run {
    pub fs: Fs<RecordingDisk>,
    pub model: Model,
    pub rng: Rng,
    /// Expected state by committed generation.
    pub expected: BTreeMap<u64, State>,
    /// (log length after the commit's second flush, generation) for each commit.
    pub waited: Vec<(usize, u64)>,
    /// The disk at the last checkpoint: every crash state starts from it.
    pub base: MemDisk,
}

fn inline_value(rng: &mut Rng) -> Vec<u8> {
    let len = 1 + rng.below(100) as usize;
    (0..len).map(|_| rng.next_u64() as u8).collect()
}

impl Run {
    /// A fresh filesystem with `keys` keys, committed, as the base of crash states.
    pub fn setup(blocks: u64, keys: u64, kind: CsumKind, seed: u64) -> Self {
        let fs = Fs::format(RecordingDisk::new(blocks), kind, [1; 16]).unwrap();
        let mut run = Run {
            fs,
            model: Model::new(),
            rng: Rng(seed),
            expected: BTreeMap::new(),
            waited: Vec::new(),
            base: MemDisk::default(),
        };
        for key in 0..keys {
            let value = inline_value(&mut run.rng);
            run.fs.put(key * 2, &value).unwrap();
            run.model.live.insert(key * 2, Value::Inline(value));
            if key % 4096 == 4095 {
                run.fs.commit().unwrap();
            }
        }
        run.commit();
        run.checkpoint();
        run
    }

    /// The disk as it is now becomes the base: the log and the history restart.
    pub fn checkpoint(&mut self) {
        self.fs.io.dev.checkpoint();
        self.base = self.fs.io.dev.disk.clone();
        self.expected.clear();
        self.waited.clear();
        self.expected.insert(self.fs.committed, self.model.state());
        self.waited.push((0, self.fs.committed));
    }

    /// A copy that continues from the same disk, model and history.
    pub fn fork(&self, seed: u64) -> Self {
        let disk = self.fs.io.dev.clone();
        Run {
            fs: Fs::mount(disk).unwrap(),
            model: self.model.clone(),
            rng: Rng(seed),
            expected: self.expected.clone(),
            waited: self.waited.clone(),
            base: self.base.clone(),
        }
    }

    fn existing(&mut self) -> Option<u64> {
        let n = self.model.live.len() as u64;
        (n > 0).then(|| {
            *self
                .model
                .live
                .keys()
                .nth(self.rng.below(n) as usize)
                .unwrap()
        })
    }

    pub fn commit(&mut self) {
        let generation = self.fs.commit().unwrap();
        self.expected.insert(generation, self.model.state());
        self.waited.push((self.fs.io.dev.log.len(), generation));
    }

    pub fn act(&mut self, action: Action) {
        match action {
            Action::Put => {
                let key = 1 + 2 * self.rng.below(1 << 40);
                let value = inline_value(&mut self.rng);
                self.fs.put(key, &value).unwrap();
                self.model.live.insert(key, Value::Inline(value));
            }
            Action::Delete => {
                if let Some(key) = self.existing() {
                    assert!(self.fs.delete(key).unwrap());
                    self.model.live.remove(&key);
                }
            }
            Action::Overwrite => {
                if let Some(key) = self.existing() {
                    let data: Vec<u8> = (0..4096).map(|_| self.rng.next_u64() as u8).collect();
                    self.fs.put_record(key, &data).unwrap();
                    self.model.live.insert(key, Value::Record(data));
                }
            }
            Action::Snapshot => {
                let id = self.fs.snapshot().unwrap();
                assert_eq!(id, self.model.next_snap);
                self.model.next_snap += 1;
                self.model.snaps.insert(id, self.model.live.clone());
            }
            Action::DropSnapshot => {
                if let Some(&id) = self.model.snaps.keys().next_back() {
                    assert!(self.fs.drop_snapshot(id).unwrap());
                    self.model.snaps.remove(&id);
                }
            }
            Action::Restore => {
                if let Some((&id, map)) = self.model.snaps.iter().next_back() {
                    self.model.live = map.clone();
                    assert!(self.fs.restore(id).unwrap());
                }
            }
            Action::Commit => self.commit(),
        }
    }
}

/// Totals over many workloads.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    pub states: u64,
    pub violations: u64,
    /// The first few violations, described.
    pub examples: Vec<String>,
    /// States whose mount fell back past the newest commit in the image.
    pub fell_back: u64,
}

impl Tally {
    pub fn add(&mut self, other: &Tally) {
        self.states += other.states;
        self.violations += other.violations;
        self.fell_back += other.fell_back;
        for example in &other.examples {
            if self.examples.len() < 10 {
                self.examples.push(example.clone());
            }
        }
    }
}

/// One crash state: writes applied over the base, in log order, and an optional
/// torn write last.
struct Crash<'a> {
    writes: Vec<&'a (u64, Rc<Block>)>,
    torn: Option<(&'a (u64, Rc<Block>), usize)>,
    /// The oldest generation the oracle accepts.
    min_generation: u64,
}

/// A flush epoch: the index of its first op, and its writes.
type Epoch = (usize, Vec<(u64, Rc<Block>)>);

/// Checks every crash state of `run`'s log against its history.
pub fn enumerate(run: &Run, cache: &mut Cache, rng: &mut Rng) -> Tally {
    let base = &run.base;
    let log = &run.fs.io.dev.log;
    let block_count = base_blocks(base);
    let slots: BTreeSet<u64> = format::all_slots(block_count).collect();
    // Epochs: (index of the first op, the writes) between flushes.
    let mut epochs: Vec<Epoch> = Vec::new();
    let mut current = (0, Vec::new());
    for (i, op) in log.iter().enumerate() {
        match op {
            Op::Write(addr, block) => current.1.push((*addr, block.clone())),
            Op::Flush => {
                let done = core::mem::replace(&mut current, (i + 1, Vec::new()));
                epochs.push(done);
            }
        }
    }
    epochs.push(current);
    let mut tally = Tally::default();
    let mut durable: Vec<&(u64, Rc<Block>)> = Vec::new();
    for (start, writes) in &epochs {
        let min_generation = run
            .waited
            .iter()
            .filter(|(at, _)| *at <= *start)
            .map(|(_, g)| *g)
            .max()
            .unwrap_or(0);
        let n = writes.len();
        // Every subset (prefixes among them) up to 12 writes; above that every
        // prefix and 4,096 random subsets.
        let mut masks: Vec<Vec<bool>> = Vec::new();
        if n <= 12 {
            for mask in 0u32..(1 << n) {
                masks.push((0..n).map(|i| mask & (1 << i) != 0).collect());
            }
        } else {
            for k in 0..=n {
                masks.push((0..n).map(|i| i < k).collect());
            }
            for _ in 0..4096 {
                masks.push((0..n).map(|_| rng.next_u64() & 1 == 1).collect());
            }
        }
        for mask in &masks {
            let mut applied = durable.clone();
            applied.extend(
                writes
                    .iter()
                    .zip(mask)
                    .filter(|(_, on)| **on)
                    .map(|(w, _)| w),
            );
            let crash = Crash {
                writes: applied,
                torn: None,
                min_generation,
            };
            judge(run, base, &crash, cache, &mut tally);
        }
        // Torn superblock slots.
        for (t, torn) in writes.iter().enumerate() {
            if !slots.contains(&torn.0) {
                continue;
            }
            let others: Vec<usize> = (0..n).filter(|&i| i != t).collect();
            let others_masks: Vec<Vec<bool>> = if others.len() <= 12 {
                (0u32..(1 << others.len()))
                    .map(|m| (0..others.len()).map(|i| m & (1 << i) != 0).collect())
                    .collect()
            } else {
                (0..64)
                    .map(|_| (0..others.len()).map(|_| rng.next_u64() & 1 == 1).collect())
                    .collect()
            };
            for mask in &others_masks {
                for step in 1..8 {
                    let mut applied = durable.clone();
                    applied.extend(
                        others
                            .iter()
                            .zip(mask)
                            .filter(|(_, on)| **on)
                            .map(|(i, _)| &writes[*i]),
                    );
                    let crash = Crash {
                        writes: applied,
                        torn: Some((torn, step * 512)),
                        min_generation,
                    };
                    judge(run, base, &crash, cache, &mut tally);
                }
            }
        }
        durable.extend(writes.iter());
    }
    tally
}

fn base_blocks(base: &MemDisk) -> u64 {
    crate::disk::BlockDev::blocks(base)
}

fn judge(run: &Run, base: &MemDisk, crash: &Crash, cache: &mut Cache, tally: &mut Tally) {
    tally.states += 1;
    let mut image = Image::new(base);
    for (addr, block) in &crash.writes {
        image.apply(*addr, block);
    }
    if let Some(((addr, block), bytes)) = crash.torn {
        image.apply_torn(*addr, block, bytes);
    }
    let verdict = (|| -> Result<u64, String> {
        let mut fs = Fs::mount(image).map_err(|e| std::format!("mount: {e:?}"))?;
        let report = check::check(&mut fs, cache).map_err(|e| std::format!("check: {e}"))?;
        let generation = fs.committed;
        if generation < crash.min_generation {
            return Err(std::format!(
                "mounted generation {generation}, older than the waited-on {}",
                crash.min_generation
            ));
        }
        let expected = run
            .expected
            .get(&generation)
            .ok_or_else(|| std::format!("generation {generation} was never committed"))?;
        let state = State {
            live: report.live,
            snaps: report.snaps,
        };
        if &state != expected {
            return Err(std::format!(
                "generation {generation}: state {state:?}, expected {expected:?}"
            ));
        }
        Ok(generation)
    })();
    match verdict {
        Ok(generation) => {
            let newest = run.expected.keys().next_back().copied().unwrap_or(0);
            if generation < newest && crash.torn.is_some() {
                tally.fell_back += 1;
            }
        }
        Err(why) => {
            tally.violations += 1;
            if tally.examples.len() < 10 {
                tally.examples.push(why);
            }
        }
    }
}

/// Every sequence of up to `len` actions, in a fixed order.
pub fn sequences(len: usize) -> Vec<Vec<Action>> {
    let mut out = alloc::vec![Vec::new()];
    let mut frontier = alloc::vec![Vec::new()];
    for _ in 0..len {
        let mut next = Vec::new();
        for seq in &frontier {
            for action in ACTIONS {
                let mut longer: Vec<Action> = seq.clone();
                longer.push(action);
                next.push(longer);
            }
        }
        out.extend(next.iter().cloned());
        frontier = next;
    }
    out
}
