//! P2 task 4's exit gate: spike S04-1's crash harness, run on kaifs proper (ADR
//! 0004). A workload runs on a recording disk beside a model; then every crash
//! state the enumerator rebuilds must mount, pass `check()`, and hold exactly one
//! committed generation, its live tree and every snapshot as the model had them,
//! never older than the last commit waited on.
//!
//! Workloads, as S04-1 set them:
//! - every sequence of up to three of {put, delete, overwrite with a 4 KiB record,
//!   snapshot, drop snapshot, restore, commit}, then a commit, on an empty tree, a
//!   1,000-key tree and a 100,000-key tree;
//! - random batches of 120 actions on 1,000 and 100,000 keys, whose epochs pass 12
//!   writes and get 4,096 random subsets each.
//!
//! The bar: at least 1,000,000 crash states, no violation. S04-1's deliberate bugs
//! must each be caught: no flush before the superblock, a replaced block reused in
//! its own transaction, snapshots ignored when freeing, and the reuse rule judged
//! at commit; reuse at N+1 must not be (ADR 0004 §3).
//!
//! Release builds run it all (a few minutes):
//! `cargo test --release -p kaifs --test crash -- --nocapture --test-threads 2`

use kaifs::check::{Cache, Report, check, map_digest};
use kaifs::crash::{Options, Oracle, Recording, Rng, Tally, enumerate};
use kaifs::format;
use kaifs::fs::{Faults, Fs, Value};
use kaifs::{CsumKind, Image, Key, MemDisk, RecordingDisk};
use std::collections::BTreeMap;
use std::time::Instant;

const UUID: [u8; 16] = *b"kaifs crash gate";

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    live: (u64, u64),
    snaps: BTreeMap<u64, (u64, u64)>,
}

#[derive(Clone, Debug, Default)]
struct Model {
    live: BTreeMap<Key, Value>,
    snaps: BTreeMap<u64, BTreeMap<Key, Value>>,
    next_snap: u64,
}

impl Model {
    fn state(&self) -> State {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Put,
    Delete,
    Overwrite,
    Snapshot,
    DropSnapshot,
    Restore,
    Commit,
}

const ACTIONS: [Action; 7] = [
    Action::Put,
    Action::Delete,
    Action::Overwrite,
    Action::Snapshot,
    Action::DropSnapshot,
    Action::Restore,
    Action::Commit,
];

struct Run {
    fs: Fs<RecordingDisk>,
    model: Model,
    rng: Rng,
    expected: BTreeMap<u64, State>,
    waited: Vec<(usize, u64)>,
    base: MemDisk,
    faults: Faults,
}

fn key(n: u64) -> Key {
    Key::new(n, 1, 0)
}

impl Run {
    fn setup(blocks: u64, keys: u64, seed: u64) -> Run {
        let fs = Fs::format(RecordingDisk::new(blocks), UUID, "crash gate").unwrap();
        let mut run = Run {
            fs,
            model: Model {
                next_snap: 1,
                ..Model::default()
            },
            rng: Rng(seed),
            expected: BTreeMap::new(),
            waited: Vec::new(),
            base: MemDisk::default(),
            faults: Faults::default(),
        };
        for n in 0..keys {
            let value = run.inline();
            run.fs.put(key(n * 2), &value).unwrap();
            run.model.live.insert(key(n * 2), Value::Inline(value));
            if n % 4096 == 4095 {
                run.fs.commit().unwrap();
            }
        }
        run.commit();
        run.checkpoint();
        run
    }

    fn checkpoint(&mut self) {
        self.fs.io.dev.checkpoint();
        self.base = self.fs.io.dev.disk.clone();
        self.expected.clear();
        self.waited.clear();
        self.expected.insert(self.fs.committed, self.model.state());
        self.waited.push((0, self.fs.committed));
    }

    /// A copy from the same disk, model and history, with `faults`.
    fn fork(&self, seed: u64, faults: Faults) -> Run {
        Run {
            fs: Fs::mount_with(self.fs.io.dev.clone(), faults).unwrap(),
            model: self.model.clone(),
            rng: Rng(seed),
            expected: self.expected.clone(),
            waited: self.waited.clone(),
            base: self.base.clone(),
            faults,
        }
    }

    fn inline(&mut self) -> Vec<u8> {
        let len = 1 + self.rng.below(100) as usize;
        (0..len).map(|_| self.rng.next_u64() as u8).collect()
    }

    fn existing(&mut self) -> Option<Key> {
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

    fn commit(&mut self) {
        self.try_act(Action::Commit).unwrap();
    }

    fn act(&mut self, action: Action) {
        self.try_act(action).unwrap();
    }

    /// One action; Err if the filesystem refused it, which only a fault causes.
    fn try_act(&mut self, action: Action) -> Result<(), kaifs::Error> {
        match action {
            Action::Put => {
                let k = key(1 + 2 * self.rng.below(1 << 40));
                let value = self.inline();
                self.fs.put(k, &value)?;
                self.model.live.insert(k, Value::Inline(value));
            }
            Action::Delete => {
                if let Some(k) = self.existing() {
                    assert!(self.fs.delete(&k)?);
                    self.model.live.remove(&k);
                }
            }
            Action::Overwrite => {
                if let Some(k) = self.existing() {
                    let data: Vec<u8> = (0..4096).map(|_| self.rng.next_u64() as u8).collect();
                    self.fs.put_record(k, &data)?;
                    self.model.live.insert(k, Value::Record(data));
                }
            }
            Action::Snapshot => {
                let id = self.fs.snapshot()?;
                assert_eq!(id, self.model.next_snap);
                self.model.next_snap += 1;
                self.model.snaps.insert(id, self.model.live.clone());
            }
            Action::DropSnapshot => {
                if let Some(&id) = self.model.snaps.keys().next_back() {
                    assert!(self.fs.drop_snapshot(id)?);
                    self.model.snaps.remove(&id);
                }
            }
            Action::Restore => {
                if let Some((&id, map)) = self.model.snaps.iter().next_back() {
                    self.model.live = map.clone();
                    assert!(self.fs.restore(id)?);
                }
            }
            Action::Commit => {
                let generation = self.fs.commit()?;
                self.expected.insert(generation, self.model.state());
                self.waited.push((self.fs.io.dev.log.len(), generation));
            }
        }
        Ok(())
    }

    fn enumerate(&self, rng: &mut Rng) -> Tally {
        let blocks = self.fs.block_count();
        let slots: Vec<u64> = format::all_slots(blocks).collect();
        let tear = |addr: u64| slots.contains(&addr);
        let recording = Recording {
            base: &self.base,
            log: &self.fs.io.dev.log,
            waited: &self.waited,
            tear: &tear,
        };
        let mut oracle = FsOracle {
            expected: &self.expected,
            faults: self.faults,
            cache: Cache::default(),
        };
        enumerate(&recording, &Options::default(), &mut oracle, rng)
    }
}

/// Mounts each crash state as `Fs::mount` does and checks it, through the memo.
struct FsOracle<'a> {
    expected: &'a BTreeMap<u64, State>,
    faults: Faults,
    cache: Cache,
}

impl FsOracle<'_> {
    fn report(&mut self, image: &mut Image<'_>) -> Result<Report, String> {
        let found = format::read_rings(image).map_err(|e| format!("rings: {e:?}"))?;
        let verifies = |image: &mut Image<'_>, ptr: &kaifs::BlockPtr| {
            ptr.is_null() || format::read_checked(image, CsumKind::Seahash64, ptr, "root").is_ok()
        };
        let sb = found
            .into_iter()
            .find(|sb| {
                !sb.volumes.is_null() && verifies(image, &sb.volumes) && verifies(image, &sb.alloc)
            })
            .ok_or("no superblock whose roots verify")?;
        if let Some(report) = self.cache.mounted(&sb) {
            return Ok(report);
        }
        let mut fs =
            Fs::mount_with(&mut *image, self.faults).map_err(|e| format!("mount: {e:?}"))?;
        if fs.committed != sb.generation {
            return Err(format!(
                "mount took {}, not {}",
                fs.committed, sb.generation
            ));
        }
        check(&mut fs, &mut self.cache).map_err(|e| format!("check: {e}"))
    }
}

impl Oracle for FsOracle<'_> {
    fn judge(&mut self, image: &mut Image<'_>, min: u64) -> Result<(), String> {
        self.cache.taint(image.overlay.keys().copied());
        let report = self.report(image)?;
        let generation = report.generation;
        if generation < min {
            return Err(format!(
                "mounted generation {generation}, older than the waited-on {min}"
            ));
        }
        let want = self
            .expected
            .get(&generation)
            .ok_or(format!("generation {generation} was never committed"))?;
        let got = State {
            live: (report.live.digest, report.live.count),
            snaps: report
                .snaps
                .iter()
                .map(|(id, t)| (*id, (t.digest, t.count)))
                .collect(),
        };
        match got == *want {
            true => Ok(()),
            false => Err(format!(
                "generation {generation}: {got:?}, expected {want:?}"
            )),
        }
    }

    fn durable(&mut self, written: &[u64]) {
        self.cache.forget(written);
    }
}

/// Every sequence of up to `len` actions.
fn sequences(len: usize) -> Vec<Vec<Action>> {
    let mut out = vec![Vec::new()];
    let mut frontier = vec![Vec::new()];
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

fn report(what: &str, tally: &Tally, start: Instant) {
    eprintln!(
        "kaifs crash: {what}: {} crash states, {} violations, {:.1} s",
        tally.states,
        tally.violations,
        start.elapsed().as_secs_f64()
    );
    for example in tally.examples.iter().take(3) {
        eprintln!("  violation: {example}");
    }
}

fn sweep(keys: u64, blocks: u64) -> Tally {
    let start = Instant::now();
    let setup = Run::setup(blocks, keys, 0xC0FFEE ^ keys);
    let mut tally = Tally::default();
    let mut rng = Rng(42);
    for (i, seq) in sequences(3).iter().enumerate() {
        let mut run = setup.fork(1000 + i as u64, Faults::default());
        for action in seq {
            run.act(*action);
        }
        run.act(Action::Commit);
        tally.add(&run.enumerate(&mut rng));
    }
    report(
        &format!("{keys:6} keys, every sequence of three"),
        &tally,
        start,
    );
    tally
}

fn batch_actions(pick: &mut Rng) -> Action {
    match pick.below(100) {
        0..=39 => Action::Put,
        40..=59 => Action::Overwrite,
        60..=74 => Action::Delete,
        75..=78 => Action::Snapshot,
        79..=80 => Action::DropSnapshot,
        81 => Action::Restore,
        _ if pick.below(6) == 0 => Action::Commit,
        _ => Action::Put,
    }
}

fn batches(keys: u64, blocks: u64, workloads: u64) -> Tally {
    let start = Instant::now();
    let setup = Run::setup(blocks, keys, 0xBA7C4 ^ keys);
    let mut tally = Tally::default();
    let mut rng = Rng(7);
    for w in 0..workloads {
        let mut run = setup.fork(5000 + w, Faults::default());
        let mut pick = Rng(77 + w);
        for _ in 0..120 {
            run.act(batch_actions(&mut pick));
        }
        run.act(Action::Commit);
        tally.add(&run.enumerate(&mut rng));
    }
    report(
        &format!("{keys:6} keys, {workloads} batches of 120"),
        &tally,
        start,
    );
    tally
}

/// S04-1's bar: zero violations over at least 1,000,000 crash states.
#[test]
fn a_million_crash_states() {
    let debug = cfg!(debug_assertions);
    let mut total = Tally::default();
    total.add(&sweep(0, 4096));
    total.add(&sweep(1000, 8192));
    if !debug {
        total.add(&sweep(100_000, 32768));
    }
    total.add(&batches(1000, 16384, if debug { 2 } else { 80 }));
    if !debug {
        total.add(&batches(100_000, 65536, 20));
    }
    eprintln!(
        "kaifs crash total: {} crash states, {} violations",
        total.states, total.violations
    );
    assert_eq!(total.violations, 0, "{:#?}", total.examples);
    if !debug {
        assert!(total.states >= 1_000_000, "only {} states", total.states);
    }
}

/// Runs `seqs` with `faults` on a 1,000-key tree.
fn faulty(faults: Faults, seqs: &[Vec<Action>]) -> Tally {
    let start = Instant::now();
    let setup = Run::setup(8192, 1000, 7);
    let mut tally = Tally::default();
    let mut rng = Rng(42);
    for (i, seq) in seqs.iter().enumerate() {
        let mut run = setup.fork(1000 + i as u64, faults);
        for action in seq {
            if let Err(e) = run.try_act(*action) {
                // The fault broke the running filesystem itself: caught.
                tally.violations += 1;
                tally
                    .examples
                    .push(format!("the workload failed at {action:?}: {e:?}"));
                break;
            }
        }
        tally.add(&run.enumerate(&mut rng));
    }
    report(&format!("{faults:?}"), &tally, start);
    tally
}

fn fault_sequences() -> Vec<Vec<Action>> {
    use Action::*;
    vec![
        vec![Put, Commit, Overwrite, Commit, Delete, Commit],
        vec![Snapshot, Commit, Overwrite, Commit, Overwrite, Commit],
        vec![
            Overwrite, Commit, Put, Commit, Overwrite, Commit, Delete, Commit,
        ],
        // S04-1's finding: snapshot, commit, change, restore, drop, commit, then
        // writes that land on any wrongly freed block.
        vec![
            Snapshot,
            Commit,
            Overwrite,
            Overwrite,
            Overwrite,
            Restore,
            DropSnapshot,
            Commit,
            Overwrite,
            Put,
            Commit,
            Overwrite,
            Overwrite,
            Commit,
            Put,
            Overwrite,
            Commit,
        ],
    ]
}

#[test]
fn the_harness_catches_each_deliberate_bug() {
    let seqs = fault_sequences();
    for faults in [
        Faults {
            skip_data_flush: true,
            ..Faults::default()
        },
        Faults {
            reuse_now: true,
            ..Faults::default()
        },
        Faults {
            ignore_snapshots: true,
            ..Faults::default()
        },
        Faults {
            decide_at_commit: true,
            ..Faults::default()
        },
    ] {
        assert!(
            faulty(faults, &seqs).violations > 0,
            "{faults:?} went unnoticed"
        );
    }
}

#[test]
fn reuse_at_n_plus_one_is_safe_and_the_clean_code_passes() {
    let seqs = fault_sequences();
    let next = Faults {
        reuse_next: true,
        ..Faults::default()
    };
    assert_eq!(faulty(next, &seqs).violations, 0);
    assert_eq!(faulty(Faults::default(), &seqs).violations, 0);
}
