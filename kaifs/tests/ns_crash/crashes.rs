//! The workloads (every short sequence of operations, and random batches) and
//! the gate's tests.

use super::*;

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
        "kaifs ns crash: {what}: {} crash states, {} violations, {:.1} s",
        tally.states,
        tally.violations,
        start.elapsed().as_secs_f64()
    );
    for example in tally.examples.iter().take(3) {
        eprintln!("  violation: {example}");
    }
}

fn sweep(files: u64, blocks: u64, len: usize) -> Tally {
    let start = Instant::now();
    let setup = Run::setup(blocks, files, 0x5EED ^ files);
    let mut tally = Tally::default();
    let mut rng = Rng(42);
    for (i, seq) in sequences(len).iter().enumerate() {
        let mut run = setup.fork(1000 + i as u64, Faults::default());
        for action in seq {
            run.act(*action);
        }
        run.act(Action::Commit);
        tally.add(&run.enumerate(&mut rng));
    }
    report(
        &format!("{files:5} files, every sequence of {len}"),
        &tally,
        start,
    );
    tally
}

fn batch_action(pick: &mut Rng) -> Action {
    match pick.below(100) {
        0..=11 => Action::Create,
        12..=17 => Action::Mkdir,
        18..=35 => Action::WriteSmall,
        36..=43 => Action::WriteBig,
        44..=53 => Action::WriteAt,
        54..=59 => Action::Truncate,
        60..=69 => Action::Rename,
        70..=77 => Action::Remove,
        78..=82 => Action::SetXattr,
        83..=84 => Action::RemoveXattr,
        85..=86 => Action::Snapshot,
        87 => Action::DropSnapshot,
        88 => Action::Restore,
        _ => Action::Commit,
    }
}

fn batches(files: u64, blocks: u64, workloads: u64) -> Tally {
    let start = Instant::now();
    let setup = Run::setup(blocks, files, 0xBA7C4 ^ files);
    let mut tally = Tally::default();
    let mut rng = Rng(7);
    for w in 0..workloads {
        let mut run = setup.fork(5000 + w, Faults::default());
        let mut pick = Rng(77 + w);
        for _ in 0..120 {
            run.act(batch_action(&mut pick));
        }
        run.act(Action::Commit);
        tally.add(&run.enumerate(&mut rng));
    }
    report(
        &format!("{files:5} files, {workloads} batches of 120"),
        &tally,
        start,
    );
    tally
}

/// The exit gate: zero violations over the namespace operations' crash states.
#[test]
fn namespace_operations_survive_every_crash_state() {
    let debug = cfg!(debug_assertions);
    let mut total = Tally::default();
    total.add(&sweep(0, 4096, if debug { 2 } else { 3 }));
    total.add(&sweep(60, 8192, if debug { 1 } else { 3 }));
    if !debug {
        total.add(&sweep(2000, 16384, 2));
    }
    total.add(&batches(60, 16384, if debug { 2 } else { 60 }));
    if !debug {
        total.add(&batches(2000, 32768, 20));
    }
    eprintln!(
        "kaifs ns crash total: {} crash states, {} violations",
        total.states, total.violations
    );
    assert_eq!(total.violations, 0, "{:#?}", total.examples);
    if !debug {
        assert!(total.states >= 1_000_000, "only {} states", total.states);
    }
}

/// S04-1's deliberate bugs, under namespace operations.
#[test]
fn the_deliberate_bugs_are_caught_under_namespace_operations() {
    use Action::*;
    let seqs = [
        vec![WriteBig, Commit, Rename, Commit, Remove, Commit],
        vec![
            Snapshot, Commit, WriteBig, WriteAt, Commit, Truncate, Commit,
        ],
        vec![
            Snapshot,
            Commit,
            WriteBig,
            Rename,
            Restore,
            DropSnapshot,
            Commit,
            WriteBig,
            Create,
            Commit,
            WriteBig,
            Commit,
        ],
    ];
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
        let start = Instant::now();
        let setup = Run::setup(8192, 60, 3);
        let mut tally = Tally::default();
        let mut rng = Rng(42);
        for (i, seq) in seqs.iter().enumerate() {
            let mut run = setup.fork(1 + i as u64, faults);
            for action in seq {
                if let Err(e) = run.try_act(*action) {
                    tally.violations += 1;
                    tally
                        .examples
                        .push(format!("the workload failed at {action:?}: {e}"));
                    break;
                }
            }
            tally.add(&run.enumerate(&mut rng));
        }
        report(&format!("{faults:?}"), &tally, start);
        assert!(tally.violations > 0, "{faults:?} went unnoticed");
    }
}

#[test]
fn a_folder_tree_of_the_setup_is_what_the_model_says() {
    let mut run = Run::setup(8192, 60, 9);
    let mut cache = Cache::default();
    check(&mut run.fs, &mut cache).unwrap();
    let ns = kaifs::ns_check::check_namespace(&mut run.fs, &mut cache).unwrap();
    let live = &ns.volumes[&SYSTEM].live;
    assert_eq!(live.digest, run.model.live.digest());
    assert_eq!(live.files, 60);
    assert_eq!(run.fs.stat(ROOT).unwrap().kind, FileType::Dir);
}
