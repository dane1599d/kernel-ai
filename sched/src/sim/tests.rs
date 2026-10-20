use super::*;
use crate::Interactive;
use crate::workload::MIX_A;

fn interactive() -> Box<dyn SchedPolicy> {
    Box::new(Interactive::new())
}

#[test]
fn a_probe_alone_takes_exactly_its_burst() {
    let mut sim = Sim::alone(MIX_A.procs, MIX_A.target, 3, interactive());
    sim.run_until(2_000 * MS);
    let events = sim.events(MIX_A.target);
    assert!(events.len() >= 30, "{} events", events.len());
    let Behavior::Probe { gap, burst } = MIX_A.procs[MIX_A.target].behavior else {
        panic!("the target is a probe");
    };
    for pair in events.windows(2) {
        assert!((gap.low..=gap.high).contains(&(pair[1].at - pair[0].at)));
    }
    // Alone, nothing delays an event: its latency is its burst.
    assert!(
        events
            .iter()
            .all(|e| (burst.low..=burst.high).contains(&e.latency_ns))
    );
}

#[test]
fn a_probe_meets_the_same_events_loaded_and_alone() {
    let mut alone = Sim::alone(MIX_A.procs, MIX_A.target, 9, interactive());
    let mut loaded = Sim::new(MIX_A.procs, 9, interactive());
    alone.run_until(3_000 * MS);
    loaded.run_until(3_000 * MS);
    let (a, b) = (alone.events(MIX_A.target), loaded.events(MIX_A.target));
    let shared = a.len().min(b.len());
    assert!(shared >= 40);
    for (x, y) in a.iter().zip(b) {
        assert_eq!(x.at, y.at);
        assert!(y.latency_ns >= x.latency_ns, "loaded can only be slower");
    }
}

#[test]
fn every_nanosecond_is_charged_and_runs_repeat() {
    let run = |seed| {
        let mut sim = Sim::new(MIX_A.procs, seed, interactive());
        sim.run_until(1_500 * MS);
        let snapshot = sim.core().snapshot(sim.now());
        let total: u64 = snapshot.threads.iter().map(|t| t.cpu_ns).sum();
        assert_eq!(total, 1_500 * MS);
        (snapshot, sim.events(MIX_A.target).to_vec())
    };
    assert_eq!(run(4), run(4));
    assert_ne!(run(4).1, run(5).1);
}

#[test]
fn wakeful_jobs_sleep_until_a_tick_and_hogs_never_block() {
    let mut sim = Sim::new(MIX_A.procs, 2, interactive());
    sim.run_until(1_000 * MS);
    let snapshot = sim.core().snapshot(sim.now());
    let build = sim.tid(MIX_A.index("build-1").unwrap()).unwrap();
    let hog = sim.tid(MIX_A.index("hog-1").unwrap()).unwrap();
    // A 2-4 ms burst then a 1 ms sleep: a few hundred wakes a second at most.
    let wakes = snapshot.threads[build].wakes;
    assert!((50..=350).contains(&wakes), "{wakes} wakes");
    assert_eq!(snapshot.threads[hog].wakes, 0);
    assert!(snapshot.threads[hog].cpu_ns > 0);
}

#[test]
fn the_generator_is_fixed() {
    let mut rng = Rng::new(0);
    assert_eq!(rng.next_u64(), 0xe220_a839_7b1d_cdaf);
    assert_eq!(rng.below(0), 0);
    assert!(Span::new(5, 5).draw(&mut rng) == 5);
}

#[test]
fn a_target_spawned_last_still_meets_the_same_events() {
    // Hogs first: the probe's first event can arrive before it ever runs.
    let procs = [
        ProcSpec {
            name: "hog-1",
            behavior: Behavior::Hog,
        },
        ProcSpec {
            name: "hog-2",
            behavior: Behavior::Hog,
        },
        ProcSpec {
            name: "hog-3",
            behavior: Behavior::Hog,
        },
        MIX_A.procs[MIX_A.target],
    ];
    let mut alone = Sim::alone(&procs, 3, 1, interactive());
    let mut loaded = Sim::new(&procs, 1, interactive());
    alone.run_until(2_000 * MS);
    loaded.run_until(2_000 * MS);
    let (a, b) = (alone.events(3), loaded.events(3));
    assert!(b.len() >= 20);
    for (x, y) in a.iter().zip(b) {
        assert_eq!(x.at, y.at);
        assert!(y.latency_ns >= x.latency_ns, "{x:?} {y:?}");
    }
}
