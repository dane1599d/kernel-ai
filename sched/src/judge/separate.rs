//! The M10 exit rule of 2026-10-21 (the founder's): the default and the AI's
//! choice each run alone, from a fresh start, and the choice wins only if
//! - it had no ejection and no runnable wait over 100 ms;
//! - and, unless the default was ejected (an ejected default counts as failing),
//!   the target's p99 is at most 0.7x the default's, background CPU at least 0.9x
//!   in total and 0.8x for every background process that had 1% or more.
//!
//! The kernel runs each side in its own boot (`test=tune-ab phase=run`); here
//! each side is its own simulation of the same seed, measured as a trial's
//! windows are.

use super::{Choice, LATENCY_FLOOR_NS, MIN_SAMPLES, TrialPlan, Unloaded, WindowMeasure};
use super::{GUARDRAIL_WAIT_NS, classes_table, percentile, permille, window};
use crate::sim::{EventRecord, Sim};
use crate::spec::SpecError;
use crate::workload::Mix;
use alloc::vec::Vec;

/// The editor's p99 at most this share of the default's, in thousandths.
pub const RATIO_MAX_PERMILLE: u64 = 700;
pub const BACKGROUND_TOTAL_MIN: u64 = 900;
pub const BACKGROUND_EACH_MIN: u64 = 800;

/// One side run alone: its windows, with the target's samples in them.
pub struct Side {
    pub windows: Vec<WindowMeasure>,
}

impl Side {
    pub fn samples(&self) -> usize {
        self.windows.iter().map(|w| w.target_excess_ns.len()).sum()
    }

    pub fn p99(&self) -> u64 {
        let mut all: Vec<u64> = self
            .windows
            .iter()
            .flat_map(|w| w.target_excess_ns.iter().copied())
            .collect();
        all.sort_unstable();
        percentile(&all, 99)
    }

    pub fn ejections(&self) -> u32 {
        self.windows.iter().map(|w| w.ejections).sum()
    }

    fn cpu(&self, index: usize) -> u64 {
        self.windows.iter().map(|w| w.cpu_ns[index]).sum()
    }

    fn span(&self) -> u64 {
        self.windows.iter().map(|w| w.to - w.from).sum()
    }

    fn max_wait(&self) -> u64 {
        self.windows
            .iter()
            .flat_map(|w| w.max_wait_ns.iter().copied())
            .max()
            .unwrap_or(0)
    }
}

/// Runs `choice` alone on `mix` from a fresh start: its labels from the first
/// instant, the warm-up, then `plan.pairs` windows.
pub fn run_side(
    mix: &Mix,
    seed: u64,
    choice: &Choice,
    plan: &TrialPlan,
    unloaded: &Unloaded,
) -> Result<Side, SpecError> {
    let mut sim = Sim::new(mix.procs, seed, choice.spec.build()?);
    for (tid, class) in classes_table(&sim, mix, choice).into_iter().enumerate() {
        sim.core_mut().set_class(tid, class);
    }
    sim.run_until(plan.warmup_ns);
    let mut windows: Vec<WindowMeasure> = (0..plan.pairs)
        .map(|_| window(&mut sim, mix, plan))
        .collect();
    sim.run_until(sim.now() + plan.tail_ns);
    let end = sim.now();
    let finished = sim.events(mix.target).iter().copied();
    let unfinished = sim.pending(mix.target).map(|at| EventRecord {
        at,
        latency_ns: end - at,
    });
    for event in finished.chain(unfinished) {
        let excess = event.latency_ns.saturating_sub(unloaded.latency(event.at));
        if let Some(w) = windows
            .iter_mut()
            .find(|w| (w.from..w.to).contains(&event.at))
        {
            w.target_excess_ns.push(excess.max(LATENCY_FLOOR_NS));
        }
    }
    Ok(Side { windows })
}

/// What the rule made of one seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The target's p99 under the choice over the default's, in thousandths: 0
    /// when the target hardly ran under the default.
    pub ratio_permille: u64,
    pub background_permille: u64,
    /// Each background process with 1% or more of the default's CPU: its index
    /// and its share under the choice, in thousandths.
    pub shares: Vec<(usize, u64)>,
    pub default_ejected: bool,
    pub failures: Vec<&'static str>,
}

impl Outcome {
    pub fn wins(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Applies the rule to `default` and `choice`, both run alone on the same seed.
pub fn judge_sides(default: &Side, choice: &Side, target: usize) -> Outcome {
    let processes = default.windows.first().map_or(0, |w| w.cpu_ns.len());
    let background = || (0..processes).filter(|&i| i != target);
    let total = |side: &Side| background().map(|i| side.cpu(i)).sum::<u64>();
    let ratio = match default.samples() >= MIN_SAMPLES {
        true => permille(choice.p99(), default.p99()),
        false => 0,
    };
    let shares: Vec<(usize, u64)> = background()
        .filter(|&i| default.cpu(i) * 100 >= default.span())
        .map(|i| (i, permille(choice.cpu(i), default.cpu(i))))
        .collect();
    let background_permille = permille(total(choice), total(default));
    let default_ejected = default.ejections() > 0;
    let mut failures = Vec::new();
    if choice.samples() < MIN_SAMPLES {
        failures.push("the target hardly ran under the choice");
    }
    if choice.ejections() > 0 {
        failures.push("the choice was ejected");
    }
    if choice.max_wait() > GUARDRAIL_WAIT_NS {
        failures.push("a wait over 100 ms under the choice");
    }
    if !default_ejected {
        if ratio > RATIO_MAX_PERMILLE {
            failures.push("the target's p99 is over 0.7x the default's");
        }
        if background_permille < BACKGROUND_TOTAL_MIN {
            failures.push("background CPU under 0.9x");
        }
        if shares.iter().any(|&(_, share)| share < BACKGROUND_EACH_MIN) {
            failures.push("a background process under 0.8x of its CPU");
        }
    }
    Outcome {
        ratio_permille: ratio,
        background_permille,
        shares,
        default_ejected,
        failures,
    }
}

/// Both sides of `seed`, judged.
pub fn run_seed(
    mix: &Mix,
    seed: u64,
    choice: &Choice,
    plan: &TrialPlan,
) -> Result<Outcome, SpecError> {
    let unloaded = Unloaded::run(mix, seed, plan);
    let default = run_side(mix, seed, &Choice::DEFAULT, plan, &unloaded)?;
    let chosen = run_side(mix, seed, choice, plan, &unloaded)?;
    Ok(judge_sides(&default, &chosen, mix.target))
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::spec::{PolicyKind, PolicySpec, Preset};
    use crate::workload::MIX_A;

    /// classes/firm with `names` labelled.
    fn firm(names: &[&str]) -> Choice {
        let mask = MIX_A.mask(names).unwrap();
        Choice::new(PolicySpec::new(PolicyKind::Classes, Preset::Firm), mask)
    }

    /// Every seed of mix A, judged, with each seed's numbers printed.
    fn outcomes(choice: &Choice) -> Vec<Outcome> {
        (1..=5)
            .map(|seed| {
                let outcome = run_seed(&MIX_A, seed, choice, &TrialPlan::STANDARD).unwrap();
                let shares: Vec<(&str, u64)> = outcome
                    .shares
                    .iter()
                    .map(|&(i, share)| (MIX_A.procs[i].name, share))
                    .collect();
                std::eprintln!(
                    "seed {seed}: editor p99 {}/1000, background {}/1000, {shares:?} {:?}",
                    outcome.ratio_permille,
                    outcome.background_permille,
                    outcome.failures
                );
                outcome
            })
            .collect()
    }

    #[test]
    fn labelling_the_builds_and_the_indexer_wins_every_seed_of_mix_a() {
        let all = outcomes(&firm(&[
            "build-1", "build-2", "build-3", "build-4", "indexer",
        ]));
        assert!(all.iter().all(Outcome::wins));
    }

    #[test]
    fn labelled_builds_keep_their_share_but_the_hogs_pay_for_the_indexer() {
        // The builds alone: the indexer then goes before them and the hogs, the
        // only work that can give way, fall near the 0.8x floor (the founder's
        // option (a), 2026-10-21: the rule stands).
        let all = outcomes(&firm(&["build-1", "build-2", "build-3", "build-4"]));
        for outcome in &all {
            assert!(outcome.ratio_permille <= RATIO_MAX_PERMILLE);
            for &(i, share) in &outcome.shares {
                if MIX_A.procs[i].name.starts_with("build") {
                    assert!(share >= BACKGROUND_EACH_MIN, "{share}");
                }
            }
        }
    }
}
