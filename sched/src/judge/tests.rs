extern crate std;

use super::*;
use crate::classes::{BOOSTED_SLICE_NS, TICK_NS};
use crate::sim::{Behavior, ProcSpec, Span};
use crate::workload::{MIX_A, MIX_B};
use alloc::vec;
use std::println;

const SEEDS: [u64; 5] = [1, 2, 3, 4, 5];

fn window(excess_us: &[u64], cpu_ms: &[u64]) -> WindowMeasure {
    WindowMeasure {
        from: 0,
        to: 3_000 * MS,
        target_excess_ns: excess_us.iter().map(|us| us * 1000).collect(),
        cpu_ns: cpu_ms.iter().map(|ms| ms * MS).collect(),
        max_wait_ns: vec![10 * MS; cpu_ms.len()],
        max_latency_ns: vec![MS; cpu_ms.len()],
        ejections: 0,
    }
}

/// Three pairs: the target's samples, then background CPU, per side.
fn pairs(base: &[u64], cand: &[u64], base_cpu: &[u64], cand_cpu: &[u64]) -> Vec<Pair> {
    (0..3)
        .map(|_| Pair {
            baseline: window(base, base_cpu),
            candidate: window(cand, cand_cpu),
        })
        .collect()
}

fn samples(value_us: u64, count: usize) -> Vec<u64> {
    vec![value_us; count]
}

#[test]
fn the_rule_keeps_a_clear_win_and_names_each_failure() {
    let (fast, slow) = (samples(500, 30), samples(5_000, 30));
    let cpu = [100, 900, 900];
    let kept = judge(&pairs(&slow, &fast, &cpu, &cpu), 0);
    assert!(kept.kept, "{kept:?}");
    assert_eq!(kept.ratio_permille(), 100);
    assert_eq!(kept.background_permille, 1000);

    let same = judge(&pairs(&slow, &samples(4_500, 30), &cpu, &cpu), 0);
    assert_eq!(same.reason, Some(Reason::NotFaster));

    let starved_total = judge(&pairs(&slow, &fast, &cpu, &[100, 800, 800]), 0);
    assert!(matches!(
        starved_total.reason,
        Some(Reason::BackgroundTotal { permille: 888 })
    ));

    let one_starved = judge(
        &pairs(&slow, &fast, &[100, 900, 900], &[100, 1_100, 700]),
        0,
    );
    assert_eq!(
        one_starved.reason,
        Some(Reason::BackgroundProcess {
            process: 2,
            permille: 777
        })
    );

    // Under 1% of the CPU in the baseline: its loss does not count.
    let tiny = judge(
        &pairs(&slow, &fast, &[100, 900, 900, 20], &[100, 900, 920, 0]),
        0,
    );
    assert!(tiny.kept, "{tiny:?}");

    let few = judge(&pairs(&samples(5_000, 6), &samples(500, 6), &cpu, &cpu), 0);
    assert_eq!(few.reason, Some(Reason::TooFewSamples));
    assert_eq!(judge(&[], 0).reason, Some(Reason::TooFewSamples));
}

#[test]
fn a_worse_pair_or_a_guardrail_reverts_even_a_pooled_win() {
    let cpu = [100, 900, 900];
    let mut mixed = pairs(&samples(5_000, 30), &samples(500, 30), &cpu, &cpu);
    mixed[1].baseline = window(&samples(400, 30), &cpu);
    assert_eq!(judge(&mixed, 0).reason, Some(Reason::PairWorse { pair: 1 }));

    let mut waited = pairs(&samples(5_000, 30), &samples(500, 30), &cpu, &cpu);
    waited[2].candidate.max_wait_ns[1] = GUARDRAIL_WAIT_NS + 1;
    assert_eq!(
        judge(&waited, 0).reason,
        Some(Reason::Guardrail {
            process: 1,
            wait_ns: GUARDRAIL_WAIT_NS + 1
        })
    );

    let mut ejected = pairs(&samples(5_000, 30), &samples(500, 30), &cpu, &cpu);
    ejected[0].candidate.ejections = 1;
    assert_eq!(judge(&ejected, 0).reason, Some(Reason::Ejected));
}

fn spec(kind: PolicyKind, preset: Preset) -> PolicySpec {
    PolicySpec::new(kind, preset)
}

fn report(mix: &Mix, label: &str, score: &Score) {
    let ratios: Vec<u64> = score.verdicts.iter().map(Verdict::ratio_permille).collect();
    let baseline_us: Vec<u64> = score
        .verdicts
        .iter()
        .map(|v| v.baseline_p99_ns / 1000)
        .collect();
    let worst: Vec<u64> = score
        .verdicts
        .iter()
        .map(|v| v.worst_process.map_or(1000, |(_, share)| share))
        .collect();
    println!(
        "mix {} {label}: kept {}/{}; interactive's target p99 {baseline_us:?} us; ratios \
         {ratios:?} (permille); worst background process {worst:?}; reasons {:?}",
        mix.name,
        score.kept(),
        score.verdicts.len(),
        score.verdicts.iter().map(|v| v.reason).collect::<Vec<_>>()
    );
}

#[test]
fn mix_a_has_headroom_and_the_judge_keeps_the_oracle_pick() {
    let plan = TrialPlan::STANDARD;
    let oracle = MIX_A.oracle_choice();
    let gentle = Choice::new(spec(PolicyKind::Classes, Preset::Gentle), oracle.background);
    let round_robin = Choice::new(spec(PolicyKind::RoundRobin, Preset::Gentle), 0);
    let editor = MIX_A.mask(&["editor"]).unwrap();
    let editor_background = Choice::new(spec(PolicyKind::Classes, Preset::Firm), editor);
    let burst_rule = Choice::new(spec(PolicyKind::Responsive, Preset::Gentle), 0);
    let choices = [oracle, gentle, round_robin, editor_background, burst_rule];
    let scores = evaluate(&MIX_A, &SEEDS, &choices, &plan).unwrap();
    for (score, label) in scores
        .iter()
        .zip(["oracle", "gentle", "rr", "editor-bg", "burst"])
    {
        report(&MIX_A, label, score);
    }
    let [oracle, gentle, round_robin, editor_background, burst_rule] = &scores[..] else {
        unreachable!();
    };
    for verdict in &oracle.verdicts {
        // Interactive leaves the editor at least 2 ms of excess latency at p99...
        assert!(verdict.baseline_p99_ns >= 2 * MS, "{verdict:?}");
        // ...and the oracle pick cuts it by at least 40% on every seed, with
        // background work holding.
        assert!(verdict.kept, "{verdict:?}");
        assert!(verdict.ratio_permille() <= 600, "{verdict:?}");
        assert!(verdict.worst_process.is_some_and(|(_, share)| share >= 800));
    }
    // On average it halves it.
    assert!(
        oracle.mean_ratio_permille() <= 500,
        "{}",
        oracle.mean_ratio_permille()
    );
    assert!(gentle.kept_everywhere());
    assert!(
        round_robin
            .verdicts
            .iter()
            .all(|v| v.reason == Some(Reason::NotFaster))
    );
    assert!(
        editor_background
            .verdicts
            .iter()
            .all(|v| v.reason == Some(Reason::NotFaster))
    );
    // The burst rule cuts latency further, but only by taking the build jobs'
    // CPU, which the judge does not allow; the names are what make the difference.
    assert!(burst_rule.verdicts.iter().all(|v| {
        v.ratio_permille() <= 200 && matches!(v.reason, Some(Reason::BackgroundProcess { .. }))
    }));
}

/// A heavier mix than A, where background builds wait long enough for the
/// floor to fire: four builds and six hogs, no indexer.
const CROWDED: Mix = Mix {
    name: "crowded",
    procs: &[
        ProcSpec {
            name: "editor",
            behavior: MIX_A.procs[0].behavior,
        },
        ProcSpec {
            name: "build-1",
            behavior: MIX_A.procs[1].behavior,
        },
        ProcSpec {
            name: "build-2",
            behavior: MIX_A.procs[1].behavior,
        },
        ProcSpec {
            name: "build-3",
            behavior: MIX_A.procs[1].behavior,
        },
        ProcSpec {
            name: "build-4",
            behavior: MIX_A.procs[1].behavior,
        },
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
        ProcSpec {
            name: "hog-4",
            behavior: Behavior::Hog,
        },
        ProcSpec {
            name: "hog-5",
            behavior: Behavior::Hog,
        },
        ProcSpec {
            name: "hog-6",
            behavior: Behavior::Hog,
        },
    ],
    target: 0,
    oracle: &[],
};

/// What the floor guarantees a background thread that woke: it is picked within
/// the preset's maximum, plus the tick it is counted from, plus a boosted slice
/// for the running thread and for each other labelled thread due with it.
#[test]
fn the_floor_bounds_a_woken_background_threads_wait_and_the_presets_differ() {
    let plan = TrialPlan::STANDARD;
    let builds = CROWDED
        .mask(&["build-1", "build-2", "build-3", "build-4"])
        .unwrap();
    let labelled = builds.count_ones() as u64;
    let mut trials = Vec::new();
    for preset in Preset::ALL {
        let choice = Choice::new(spec(PolicyKind::Classes, preset), builds);
        let bound = preset.numbers().1 + TICK_NS + labelled * BOOSTED_SLICE_NS;
        let alone = Unloaded::run(&CROWDED, 1, &plan);
        let trial = run_trial(&CROWDED, 1, &Choice::DEFAULT, &choice, &plan, &alone).unwrap();
        let longest = trial
            .pairs
            .iter()
            .flat_map(|pair| (1..=4).map(move |i| pair.candidate.max_latency_ns[i]))
            .max()
            .unwrap();
        println!("crowded {preset:?}: longest woken background wait {longest} ns, bound {bound}");
        assert!(longest <= bound, "{preset:?}: {longest} ns");
        trials.push((longest, trial.pairs));
    }
    // The floor fired: gentle's tighter maximum changed the schedule.
    assert_ne!(trials[0].1, trials[1].1);
    assert!(trials[0].0 < trials[1].0);
}

#[test]
fn a_starving_candidate_trips_the_guardrail_and_an_ejected_baseline_runs_nothing() {
    let plan = TrialPlan::STANDARD;
    // Thirteen hogs: round-robin's 10 ms turns keep each waiting over 100 ms.
    const HOGS: Mix = Mix {
        name: "hogs",
        procs: &[
            ProcSpec {
                name: "editor",
                behavior: MIX_A.procs[0].behavior,
            },
            ProcSpec {
                name: "h1",
                behavior: Behavior::Hog,
            },
            ProcSpec {
                name: "h2",
                behavior: Behavior::Hog,
            },
            ProcSpec {
                name: "h3",
                behavior: Behavior::Hog,
            },
            ProcSpec {
                name: "h4",
                behavior: Behavior::Hog,
            },
            ProcSpec {
                name: "h5",
                behavior: Behavior::Hog,
            },
            ProcSpec {
                name: "h6",
                behavior: Behavior::Hog,
            },
            ProcSpec {
                name: "h7",
                behavior: Behavior::Hog,
            },
            ProcSpec {
                name: "h8",
                behavior: Behavior::Hog,
            },
            ProcSpec {
                name: "h9",
                behavior: Behavior::Hog,
            },
            ProcSpec {
                name: "h10",
                behavior: Behavior::Hog,
            },
            ProcSpec {
                name: "h11",
                behavior: Behavior::Hog,
            },
            ProcSpec {
                name: "h12",
                behavior: Behavior::Hog,
            },
            ProcSpec {
                name: "h13",
                behavior: Behavior::Hog,
            },
        ],
        target: 0,
        oracle: &[],
    };
    let round_robin = Choice::new(spec(PolicyKind::RoundRobin, Preset::Gentle), 0);
    let alone = Unloaded::run(&HOGS, 1, &plan);
    let trial = run_trial(&HOGS, 1, &Choice::DEFAULT, &round_robin, &plan, &alone).unwrap();
    assert!(
        matches!(trial.verdict.reason, Some(Reason::Guardrail { wait_ns, .. }) if wait_ns > GUARDRAIL_WAIT_NS),
        "{:?}",
        trial.verdict
    );
    // Six jobs that always have a wake pending starve the hog under Interactive
    // itself: the watchdog ejects the baseline, so no candidate can be judged.
    const STARVES: Mix = Mix {
        name: "starves",
        procs: &[
            ProcSpec {
                name: "editor",
                behavior: MIX_A.procs[0].behavior,
            },
            ProcSpec {
                name: "w1",
                behavior: WAKER,
            },
            ProcSpec {
                name: "w2",
                behavior: WAKER,
            },
            ProcSpec {
                name: "w3",
                behavior: WAKER,
            },
            ProcSpec {
                name: "w4",
                behavior: WAKER,
            },
            ProcSpec {
                name: "w5",
                behavior: WAKER,
            },
            ProcSpec {
                name: "w6",
                behavior: WAKER,
            },
            ProcSpec {
                name: "hog",
                behavior: Behavior::Hog,
            },
        ],
        target: 0,
        oracle: &[],
    };
    const WAKER: Behavior = Behavior::Wakeful {
        run: Span::new(MS, 1_900 * US),
        sleep: Span::new(MS, MS),
    };
    let responsive = Choice::new(spec(PolicyKind::Responsive, Preset::Gentle), 0);
    let alone = Unloaded::run(&STARVES, 1, &plan);
    let trial = run_trial(&STARVES, 1, &Choice::DEFAULT, &responsive, &plan, &alone).unwrap();
    assert_eq!(trial.verdict.reason, Some(Reason::NotRun));
    assert!(!trial.verdict.kept);
}

#[test]
fn on_the_control_mix_classes_changes_nothing_and_is_not_kept() {
    // Interactive is already as good as anything here: the editor preempts a hog
    // at once, so both sides sit on the 100 us floor and the ratio is 1.
    let plan = TrialPlan::STANDARD;
    let hogs = MIX_B.mask(&["hog-1", "hog-2", "hog-3"]).unwrap();
    let classes = Choice::new(spec(PolicyKind::Classes, Preset::Firm), hogs);
    let scores = evaluate(&MIX_B, &SEEDS, &[classes], &plan).unwrap();
    report(&MIX_B, "classes", &scores[0]);
    assert_eq!(MIX_B.oracle_choice(), Choice::DEFAULT);
    for verdict in &scores[0].verdicts {
        assert!(
            (900..=1100).contains(&verdict.ratio_permille()),
            "{verdict:?}"
        );
        assert!(!verdict.kept);
    }
}

#[test]
fn trials_repeat_exactly_for_a_seed() {
    let plan = TrialPlan::STANDARD;
    let choice = MIX_A.oracle_choice();
    let alone = Unloaded::run(&MIX_A, 7, &plan);
    let first = run_trial(&MIX_A, 7, &Choice::DEFAULT, &choice, &plan, &alone).unwrap();
    let again = run_trial(&MIX_A, 7, &Choice::DEFAULT, &choice, &plan, &alone).unwrap();
    assert_eq!(first.pairs, again.pairs);
    assert_eq!(first.verdict, again.verdict);
    let other = Unloaded::run(&MIX_A, 8, &plan);
    let different = run_trial(&MIX_A, 8, &Choice::DEFAULT, &choice, &plan, &other).unwrap();
    assert_ne!(first.pairs, different.pairs);
}

#[test]
fn every_candidate_is_tried_and_the_best_is_reported() {
    let plan = TrialPlan::STANDARD;
    let candidates = candidates(&MIX_A);
    // 3 label-free policies, then 2 presets x 2^5 label sets (4 builds, indexer).
    assert_eq!(candidates.len(), 3 + 2 * 32);
    let scores = evaluate(&MIX_A, &SEEDS, &candidates, &plan).unwrap();
    let best = best(&scores).expect("something is kept on every seed");
    report(&MIX_A, "exhaustive best", best);
    let kept: Vec<&Score> = scores.iter().filter(|s| s.kept_everywhere()).collect();
    println!(
        "mix A: {} of {} candidates kept on every seed",
        kept.len(),
        scores.len()
    );
    // The best is the build jobs and the indexer labelled background. On mix A
    // the presets act alike (the floor never fires and the wakes cut the hogs'
    // turns short of either slice), so gentle wins the tie by coming first.
    let oracle = MIX_A.oracle_choice().background;
    assert_eq!(
        best.choice,
        Choice::new(spec(PolicyKind::Classes, Preset::Gentle), oracle)
    );
    let firm = scores
        .iter()
        .find(|s| s.choice == MIX_A.oracle_choice())
        .unwrap();
    assert_eq!(firm.verdicts, best.verdicts);
    assert!(best.mean_ratio_permille() <= MIX_A_ORACLE_RATIO_LIMIT);
    // Labelling the builds alone is not kept everywhere: the indexer's wakes then
    // go before theirs and the hogs pay for its extra CPU.
    let builds = MIX_A
        .mask(&["build-1", "build-2", "build-3", "build-4"])
        .unwrap();
    assert!(kept.iter().all(|s| s.choice.background != builds));
}

/// The oracle's measured mean ratio is about 0.47; the best can be no worse.
const MIX_A_ORACLE_RATIO_LIMIT: u64 = 500;
