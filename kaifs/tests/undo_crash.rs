//! P2 task 6's crash gate: actions across volumes, with their undo snapshots and
//! UNDO records, restores of earlier actions, aborts and dropped snapshots (ADR
//! 0004 §3-5). After each commit the writer checks itself and keeps what every
//! tree of every volume held; then every crash state the enumerator rebuilds
//! must mount, pass `check()` and the namespace and UNDO rules on every volume,
//! and hold exactly one committed generation's trees, never older than the last
//! commit waited on. An action that changes two volumes is in both or in
//! neither.
//!
//! What it shows is crash consistency: what a restore should put back is
//! `tests/undo.rs`'s, against its own expectations.
//!
//! Release builds run it all:
//! `cargo test --release -p kaifs --test undo_crash -- --nocapture --test-threads 2`

use kaifs::check::{Cache, check};
use kaifs::crash::{Options, Oracle, Recording, Rng, Tally, enumerate};
use kaifs::format;
use kaifs::fs::{Faults, HOME, MODELS, SYSTEM};
use kaifs::ns_check::{Digest, check_namespace, check_tree};
use kaifs::{CsumKind, Fs, Image, MemDisk, ROOT, RecordingDisk};
use std::collections::BTreeMap;
use std::time::Instant;

const UUID: [u8; 16] = *b"kaifs undo crash";
const VOLS: [u64; 3] = [SYSTEM, HOME, MODELS];
const NAMES: [&str; 6] = ["a", "b", "c", "notes", "photo", "log"];

/// Every tree of every volume: the live tree's digest, and each snapshot's.
type State = BTreeMap<u64, (Digest, BTreeMap<u64, Digest>)>;

fn state(fs: &mut Fs<RecordingDisk>) -> Result<State, String> {
    let mut cache = Cache::default();
    check(fs, &mut cache)?;
    let ns = check_namespace(fs, &mut cache)?;
    Ok(ns
        .volumes
        .iter()
        .map(|(id, v)| {
            let snaps = v.snaps.iter().map(|(s, r)| (*s, r.digest)).collect();
            (*id, (v.live.digest, snaps))
        })
        .collect())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// An action of a few changes over random volumes.
    Act,
    /// The same changes, outside any action.
    Plain,
    /// An action that restores an earlier action of a random volume.
    Restore,
    /// An action that changes things and is dropped.
    Abort,
    /// A random volume's oldest undo snapshot dropped, with its record.
    Drop,
    /// A named snapshot of a random volume.
    Snapshot,
}

struct Run {
    fs: Option<Fs<RecordingDisk>>,
    rng: Rng,
    expected: BTreeMap<u64, State>,
    waited: Vec<(usize, u64)>,
    base: MemDisk,
    faults: Faults,
    next_action: u64,
}

impl Run {
    fn fs(&mut self) -> &mut Fs<RecordingDisk> {
        self.fs.as_mut().expect("a filesystem")
    }

    fn setup(blocks: u64, files: u64, seed: u64) -> Run {
        let fs = Fs::mkfs(RecordingDisk::new(blocks), UUID, "undo crash gate").unwrap();
        let mut run = Run {
            fs: Some(fs),
            rng: Rng(seed),
            expected: BTreeMap::new(),
            waited: Vec::new(),
            base: MemDisk::default(),
            faults: Faults::default(),
            next_action: 1,
        };
        for vol in VOLS {
            let fs = run.fs();
            fs.set_volume(vol).unwrap();
            let docs = fs.mkdir(ROOT, "docs").unwrap();
            for n in 0..files {
                let dir = if n % 2 == 0 { ROOT } else { docs };
                let f = fs.create(dir, &format!("seed{n}")).unwrap();
                fs.write_file(f, &vec![n as u8; (n as usize * 37) % 5000])
                    .unwrap();
            }
        }
        run.fs().commit().unwrap();
        run.checkpoint();
        run
    }

    fn checkpoint(&mut self) {
        let fs = self.fs.as_mut().unwrap();
        fs.io.dev.checkpoint();
        self.base = fs.io.dev.disk.clone();
        let committed = fs.committed;
        let now = state(fs).unwrap();
        self.expected.clear();
        self.waited.clear();
        self.expected.insert(committed, now);
        self.waited.push((0, committed));
    }

    fn fork(&self, seed: u64, faults: Faults) -> Run {
        let dev = self.fs.as_ref().unwrap().io.dev.clone();
        Run {
            fs: Some(Fs::mount_with(dev, faults).unwrap()),
            rng: Rng(seed),
            expected: self.expected.clone(),
            waited: self.waited.clone(),
            base: self.base.clone(),
            faults,
            next_action: 1,
        }
    }

    fn pick(&mut self, n: usize) -> usize {
        self.rng.below(n as u64) as usize
    }

    /// One change to a random volume; refusals are fine.
    fn change(&mut self) {
        let vol = VOLS[self.pick(3)];
        let (name, other) = (NAMES[self.pick(6)], NAMES[self.pick(6)]);
        let what = self.pick(6);
        let len = [0, 40, 1500, 4096, 140_000][self.pick(5)];
        let data: Vec<u8> = (0..len).map(|_| self.rng.next_u64() as u8).collect();
        let in_docs = self.pick(2) == 0;
        let fs = self.fs();
        fs.set_volume(vol).unwrap();
        let docs = fs.resolve("/docs").ok();
        let dir = match (in_docs, docs) {
            (true, Some(d)) => d,
            _ => ROOT,
        };
        let _ = match what {
            0 | 1 => match fs.lookup(dir, name).unwrap() {
                Some(e) if e.kind == kaifs::FileType::File => fs.write_file(e.ino, &data),
                Some(_) => Ok(()),
                None => fs.create(dir, name).and_then(|f| fs.write_file(f, &data)),
            },
            2 => fs.remove(dir, name),
            3 => fs.rename(dir, name, docs.unwrap_or(ROOT), other),
            4 => fs.mkdir(dir, name).map(|_| ()),
            _ => fs.set_xattr(dir, "user.note", &data[..data.len().min(50)]),
        };
    }

    fn commit(&mut self) -> Result<(), String> {
        let fs = self.fs();
        let generation = fs.commit().map_err(|e| format!("commit: {e:?}"))?;
        let now = state(fs).map_err(|e| format!("the writer's own check: {e}"))?;
        let at = fs.io.dev.log.len();
        self.expected.insert(generation, now);
        self.waited.push((at, generation));
        Ok(())
    }

    fn step(&mut self, step: Step) -> Result<(), String> {
        let id = self.next_action;
        self.next_action += 1;
        match step {
            Step::Act | Step::Plain | Step::Abort => {
                if step != Step::Plain {
                    self.fs().begin(id).map_err(|e| format!("begin: {e:?}"))?;
                }
                for _ in 0..1 + self.pick(4) {
                    self.change();
                }
                if step == Step::Abort {
                    let fs = self.fs.take().unwrap();
                    self.fs = Some(fs.abort().map_err(|e| format!("abort: {e:?}"))?);
                    return Ok(());
                }
            }
            Step::Restore => {
                let vol = VOLS[self.pick(3)];
                let fs = self.fs();
                fs.set_volume(vol).unwrap();
                let records = fs.undo_records().map_err(|e| format!("{e:?}"))?;
                if records.is_empty() {
                    return Ok(());
                }
                let txn = records[self.pick(records.len())].txn;
                let fs = self.fs();
                fs.begin(id).map_err(|e| format!("begin: {e:?}"))?;
                fs.restore(txn).map_err(|e| format!("restore: {e:?}"))?;
            }
            Step::Drop => {
                let vol = VOLS[self.pick(3)];
                let fs = self.fs();
                fs.set_volume(vol).unwrap();
                let oldest = fs
                    .snapshots()
                    .iter()
                    .find(|(_, s)| s.kind == kaifs::vol::SnapKind::Undo)
                    .map(|(id, _)| *id);
                if let Some(sid) = oldest {
                    fs.drop_snapshot(sid).map_err(|e| format!("{e:?}"))?;
                }
            }
            Step::Snapshot => {
                let vol = VOLS[self.pick(3)];
                let fs = self.fs();
                fs.set_volume(vol).unwrap();
                fs.snapshot().map_err(|e| format!("{e:?}"))?;
            }
        }
        self.commit()
    }

    fn enumerate(&self, rng: &mut Rng) -> Tally {
        let fs = self.fs.as_ref().unwrap();
        let slots: Vec<u64> = format::all_slots(fs.block_count()).collect();
        let tear = |addr: u64| slots.contains(&addr);
        let recording = Recording {
            base: &self.base,
            log: &fs.io.dev.log,
            waited: &self.waited,
            tear: &tear,
        };
        let mut oracle = UndoOracle {
            expected: &self.expected,
            faults: self.faults,
            cache: Cache::default(),
        };
        enumerate(&recording, &Options::default(), &mut oracle, rng)
    }
}

/// Mounts each crash state, checks it through the memo, and compares every
/// volume's trees with the generation it mounted.
struct UndoOracle<'a> {
    expected: &'a BTreeMap<u64, State>,
    faults: Faults,
    cache: Cache,
}

impl UndoOracle<'_> {
    fn state(&mut self, image: &mut Image<'_>) -> Result<(u64, State), String> {
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
        let mut mounted = None;
        let report = match self.cache.mounted(&sb) {
            Some(report) => report,
            None => {
                let mut fs = Fs::mount_with(&mut *image, self.faults)
                    .map_err(|e| format!("mount: {e:?}"))?;
                if fs.committed != sb.generation {
                    return Err(format!(
                        "mount took {}, not {}",
                        fs.committed, sb.generation
                    ));
                }
                let report = check(&mut fs, &mut self.cache).map_err(|e| format!("check: {e}"))?;
                // The UNDO records are read only by a mount.
                check_namespace(&mut fs, &mut self.cache).map_err(|e| format!("namespace: {e}"))?;
                mounted = Some(fs);
                report
            }
        };
        let mut out = State::new();
        for (vol, trees) in &report.volumes {
            let mut all = vec![(None, trees.live)];
            all.extend(trees.snaps.iter().map(|(s, t)| (Some(*s), *t)));
            let mut snaps = BTreeMap::new();
            let mut live = Digest::default();
            for (sid, tree) in all {
                let ns =
                    match self.cache.namespace(&tree.root).filter(|ns| {
                        ns.max_oid < tree.next_oid && ns.max_changed <= tree.generation
                    }) {
                        Some(ns) => ns,
                        None => {
                            if mounted.is_none() {
                                mounted = Some(
                                    Fs::mount_with(&mut *image, self.faults)
                                        .map_err(|e| format!("mount: {e:?}"))?,
                                );
                            }
                            let fs = mounted.as_mut().unwrap();
                            check_tree(
                                fs,
                                &tree.root,
                                tree.next_oid,
                                tree.generation,
                                &mut self.cache,
                            )
                            .map_err(|e| format!("volume {vol}, {sid:?}: {e}"))?
                        }
                    };
                match sid {
                    None => live = ns.digest,
                    Some(sid) => {
                        snaps.insert(sid, ns.digest);
                    }
                }
            }
            out.insert(*vol, (live, snaps));
        }
        Ok((report.generation, out))
    }
}

impl Oracle for UndoOracle<'_> {
    fn judge(&mut self, image: &mut Image<'_>, min: u64) -> Result<(), String> {
        self.cache.taint(image.overlay.keys().copied());
        let (generation, got) = self.state(image)?;
        if generation < min {
            return Err(format!(
                "mounted generation {generation}, older than the waited-on {min}"
            ));
        }
        let want = self
            .expected
            .get(&generation)
            .ok_or(format!("generation {generation} was never committed"))?;
        match got == *want {
            true => Ok(()),
            false => Err(format!(
                "generation {generation} holds other trees than were committed"
            )),
        }
    }

    fn durable(&mut self, written: &[u64]) {
        self.cache.forget(written);
    }
}

const STEPS: [Step; 6] = [
    Step::Act,
    Step::Plain,
    Step::Restore,
    Step::Abort,
    Step::Drop,
    Step::Snapshot,
];

fn report(what: &str, tally: &Tally, start: Instant) {
    eprintln!(
        "kaifs undo crash: {what}: {} crash states, {} violations, {:.1} s",
        tally.states,
        tally.violations,
        start.elapsed().as_secs_f64()
    );
    for example in tally.examples.iter().take(3) {
        eprintln!("  violation: {example}");
    }
}

/// Runs `steps` on a fork of `setup`; a step the filesystem fails counts as a
/// violation, as only a fault causes one.
fn run(setup: &Run, seed: u64, faults: Faults, steps: &[Step], rng: &mut Rng) -> Tally {
    let mut run = setup.fork(seed, faults);
    let mut tally = Tally::default();
    for step in steps {
        if let Err(e) = run.step(*step) {
            tally.violations += 1;
            tally
                .examples
                .push(format!("the workload failed at {step:?}: {e}"));
            break;
        }
    }
    tally.add(&run.enumerate(rng));
    tally
}

fn sweep(files: u64, blocks: u64, len: usize) -> Tally {
    let start = Instant::now();
    let setup = Run::setup(blocks, files, 0x5EED ^ files);
    let mut tally = Tally::default();
    let mut rng = Rng(42);
    let mut seqs: Vec<Vec<Step>> = vec![Vec::new()];
    for _ in 0..len {
        seqs = seqs
            .iter()
            .flat_map(|s| {
                STEPS.iter().map(move |step| {
                    let mut longer = s.clone();
                    longer.push(*step);
                    longer
                })
            })
            .collect();
    }
    for (i, seq) in seqs.iter().enumerate() {
        // Each sequence starts with an action, so a restore has one to undo.
        let mut steps = vec![Step::Act];
        steps.extend(seq);
        tally.add(&run(
            &setup,
            1000 + i as u64,
            Faults::default(),
            &steps,
            &mut rng,
        ));
    }
    report(
        &format!("{files:4} files a volume, every sequence of {len} after an action"),
        &tally,
        start,
    );
    tally
}

fn batches(files: u64, blocks: u64, workloads: u64) -> Tally {
    let start = Instant::now();
    let setup = Run::setup(blocks, files, 0xBA7C4 ^ files);
    let mut tally = Tally::default();
    let mut rng = Rng(7);
    for w in 0..workloads {
        let mut pick = Rng(77 + w);
        let steps: Vec<Step> = (0..40)
            .map(|_| match pick.below(100) {
                0..=49 => Step::Act,
                50..=59 => Step::Plain,
                60..=74 => Step::Restore,
                75..=84 => Step::Abort,
                85..=92 => Step::Drop,
                _ => Step::Snapshot,
            })
            .collect();
        tally.add(&run(&setup, 5000 + w, Faults::default(), &steps, &mut rng));
    }
    report(
        &format!("{files:4} files a volume, {workloads} batches of 40 steps"),
        &tally,
        start,
    );
    tally
}

/// The gate: zero violations over the crash states of actions, restores and
/// aborts across volumes.
#[test]
fn actions_restores_and_aborts_survive_every_crash_state() {
    let debug = cfg!(debug_assertions);
    let mut total = Tally::default();
    total.add(&sweep(4, 8192, if debug { 1 } else { 3 }));
    total.add(&batches(40, 16384, if debug { 2 } else { 40 }));
    if !debug {
        total.add(&sweep(400, 32768, 2));
        total.add(&batches(400, 32768, 10));
    }
    eprintln!(
        "kaifs undo crash total: {} crash states, {} violations",
        total.states, total.violations
    );
    assert_eq!(total.violations, 0, "{:#?}", total.examples);
    if !debug {
        assert!(total.states >= 1_000_000, "only {} states", total.states);
    }
}

/// An action that does not count as a snapshot while open lets its undo
/// snapshot's blocks be freed and written over: caught. So are S04-1's bugs,
/// under actions.
#[test]
fn the_deliberate_bugs_are_caught_under_actions() {
    use Step::*;
    let setup = Run::setup(8192, 40, 3);
    let seqs = [
        vec![Act, Act, Act, Act, Restore, Act, Act, Restore, Act],
        vec![Act, Plain, Act, Drop, Act, Act, Restore, Act],
    ];
    for faults in [
        Faults {
            unpinned_action: true,
            ..Faults::default()
        },
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
    ] {
        let start = Instant::now();
        let mut tally = Tally::default();
        let mut rng = Rng(42);
        for (i, seq) in seqs.iter().enumerate() {
            for seed in 0..if cfg!(debug_assertions) { 1 } else { 6 } {
                tally.add(&run(&setup, 100 * i as u64 + seed, faults, seq, &mut rng));
            }
        }
        report(&format!("{faults:?}"), &tally, start);
        assert!(tally.violations > 0, "{faults:?} went unnoticed");
    }
}
