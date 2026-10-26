//! S04-1's crash states: every sequence of up to 3 operations on an empty, a
//! 1,000-key and a 100,000-key tree, plus random batch workloads.
//! Fast: cargo test --release -p spike-kaifs --test crash -- --nocapture

use spike_kaifs::CsumKind;
use spike_kaifs::check::Cache;
use spike_kaifs::crash::{Rng, Run, Tally, enumerate, sequences};
use std::time::Instant;

fn sweep(keys: u64, blocks: u64, kind: CsumKind) -> Tally {
    let start = Instant::now();
    let setup = Run::setup(blocks, keys, kind, 0xC0FFEE ^ keys);
    let mut cache = Cache::default();
    let mut rng = Rng(42);
    let mut tally = Tally::default();
    let seqs = sequences(3);
    for (i, seq) in seqs.iter().enumerate() {
        let mut run = setup.fork(1000 + i as u64);
        for action in seq {
            run.act(*action);
        }
        tally.add(&enumerate(&run, &mut cache, &mut rng));
    }
    eprintln!(
        "kaifs crash: {keys:6} keys, {kind:?}: {} sequences, {} states, {} violations, {} fell back past a torn slot, {:.1} s",
        seqs.len(),
        tally.states,
        tally.violations,
        tally.fell_back,
        start.elapsed().as_secs_f64()
    );
    for example in &tally.examples {
        eprintln!("  violation: {example}");
    }
    tally
}

#[test]
fn empty_tree_every_sequence_of_three() {
    let tally = sweep(0, 4096, CsumKind::Seahash);
    assert_eq!(tally.violations, 0);
}

/// The harness must see each deliberate bug.
fn faulty(faults: spike_kaifs::fs::Faults, seqs: &[Vec<spike_kaifs::crash::Action>]) -> Tally {
    let mut setup = Run::setup(4096, 1000, CsumKind::Seahash, 7);
    setup.fs.faults = faults;
    let mut cache = Cache::default();
    let mut rng = Rng(42);
    let mut tally = Tally::default();
    for (i, seq) in seqs.iter().enumerate() {
        let mut run = setup.fork(1000 + i as u64);
        run.fs.faults = faults;
        run.fs.io.reuse_now = faults.reuse_now;
        run.fs.set_ignore_snapshots(faults.ignore_snapshots);
        for action in seq {
            run.act(*action);
        }
        tally.add(&enumerate(&run, &mut cache, &mut rng));
    }
    eprintln!(
        "kaifs fault {faults:?}: {} states, {} violations; first: {:?}",
        tally.states,
        tally.violations,
        tally.examples.first()
    );
    tally
}

#[test]
fn the_harness_catches_deliberate_bugs() {
    use spike_kaifs::crash::Action::*;
    use spike_kaifs::fs::Faults;
    let seqs = vec![
        vec![Put, Commit, Overwrite, Commit, Delete, Commit],
        vec![Snapshot, Commit, Overwrite, Commit, Overwrite, Commit],
        vec![
            Overwrite, Commit, Put, Commit, Overwrite, Commit, Delete, Commit,
        ],
    ];
    let skip = faulty(
        Faults {
            skip_data_flush: true,
            ..Faults::default()
        },
        &seqs,
    );
    assert!(skip.violations > 0);
    let now = faulty(
        Faults {
            reuse_now: true,
            ..Faults::default()
        },
        &seqs,
    );
    assert!(now.violations > 0);
    // Reuse one commit later is safe with two flushes per commit.
    let next = faulty(
        Faults {
            reuse_next: true,
            ..Faults::default()
        },
        &seqs,
    );
    assert_eq!(next.violations, 0);
    let snaps = faulty(
        Faults {
            ignore_snapshots: true,
            ..Faults::default()
        },
        &seqs,
    );
    assert!(snaps.violations > 0);
    let none = faulty(Faults::default(), &seqs);
    assert_eq!(none.violations, 0);
}

/// Random batch workloads: many writes per commit, so flush epochs pass 12
/// writes and each gets 4,096 random subsets.
fn batches(keys: u64, blocks: u64, kind: CsumKind, workloads: u64, actions: u64) -> Tally {
    use spike_kaifs::crash::Action::*;
    let start = Instant::now();
    let setup = Run::setup(blocks, keys, kind, 0xBA7C4 ^ keys);
    let mut cache = Cache::default();
    let mut rng = Rng(7);
    let mut tally = Tally::default();
    for w in 0..workloads {
        let mut run = setup.fork(5000 + w);
        let mut pick = Rng(77 + w);
        for _ in 0..actions {
            let action = match pick.below(100) {
                0..=39 => Put,
                40..=59 => Overwrite,
                60..=74 => Delete,
                75..=78 => Snapshot,
                79..=80 => DropSnapshot,
                81 => Restore,
                _ => {
                    if pick.below(6) == 0 {
                        Commit
                    } else {
                        Put
                    }
                }
            };
            run.act(action);
        }
        run.act(Commit);
        tally.add(&enumerate(&run, &mut cache, &mut rng));
    }
    eprintln!(
        "kaifs batches: {keys:6} keys, {kind:?}: {workloads} workloads of {actions} actions, {} states, {} violations, {:.1} s",
        tally.states,
        tally.violations,
        start.elapsed().as_secs_f64()
    );
    for example in &tally.examples {
        eprintln!("  violation: {example}");
    }
    tally
}

/// S04-1's bar: zero violations over at least 1,000,000 crash states.
#[test]
fn a_million_crash_states() {
    let mut total = Tally::default();
    total.add(&sweep(0, 4096, CsumKind::Seahash));
    total.add(&sweep(1000, 8192, CsumKind::Seahash));
    total.add(&sweep(100_000, 32768, CsumKind::Seahash));
    total.add(&sweep(1000, 8192, CsumKind::Sha256));
    total.add(&batches(1000, 16384, CsumKind::Seahash, 80, 120));
    total.add(&batches(100_000, 65536, CsumKind::Seahash, 20, 120));
    eprintln!(
        "kaifs total: {} crash states, {} violations",
        total.states, total.violations
    );
    assert_eq!(total.violations, 0, "{:#?}", total.examples);
    assert!(total.states >= 1_000_000, "only {} states", total.states);
}
