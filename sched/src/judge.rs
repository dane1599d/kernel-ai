//! The trial judge: a policy change is kept only if measurement says so.
//!
//! A trial alternates baseline and candidate windows (three pairs of 3 s), and
//! drops the first 200 ms of each, after the switch. The target's latency is taken
//! per event, from the time the event was scheduled, as the excess over the same
//! event in a run of the target alone with the same seed, floored at 100 us; exact
//! samples, so 0.8x is a real comparison. The candidate is kept only if:
//! - no candidate window had a runnable wait over 100 ms or an ejection (the
//!   guardrail, which in the kernel reverts at once);
//! - the target's pooled p99 is at most 0.8x the baseline's;
//! - no pair is worse;
//! - background CPU is at least 0.9x the baseline in total, and at least 0.8x for
//!   every background process that had at least 1% of the CPU.
//!
//! The target is the founder's choice and the background is everything else, so a
//! label cannot change who is judged. All arithmetic is in integers.

use crate::classes::Class;
use crate::sim::{EventRecord, MS, Sim, US};
use crate::spec::{PolicyKind, PolicySpec, Preset, SpecError};
use crate::stats::percentile;
use crate::workload::Mix;
use crate::{Interactive, MAX_THREADS};
use alloc::boxed::Box;
use alloc::vec::Vec;

pub const DISCARD_NS: u64 = 200 * MS;
pub const LATENCY_FLOOR_NS: u64 = 100 * US;
pub const GUARDRAIL_WAIT_NS: u64 = 100 * MS;
/// Fewer pooled target samples than this on either side and nothing is kept.
pub const MIN_SAMPLES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrialPlan {
    pub warmup_ns: u64,
    pub window_ns: u64,
    pub pairs: usize,
    /// Run after the last window, so its events can finish.
    pub tail_ns: u64,
}

impl TrialPlan {
    pub const STANDARD: TrialPlan = TrialPlan {
        warmup_ns: 1_000 * MS,
        window_ns: 3_000 * MS,
        pairs: 3,
        tail_ns: 200 * MS,
    };

    pub fn total_ns(&self) -> u64 {
        self.warmup_ns + 2 * self.pairs as u64 * self.window_ns + self.tail_ns
    }
}

/// A candidate: a spec and the processes labelled background, as a mask over the
/// mix's process indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    pub spec: PolicySpec,
    pub background: u32,
}

impl Choice {
    pub const DEFAULT: Choice = Choice {
        spec: PolicySpec::DEFAULT,
        background: 0,
    };

    pub const fn new(spec: PolicySpec, background: u32) -> Self {
        Self { spec, background }
    }

    pub fn labelled(&self, index: usize) -> bool {
        index < 32 && self.background & (1 << index) != 0
    }
}

/// What one window measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowMeasure {
    pub from: u64,
    pub to: u64,
    /// Excess latency of each target event scheduled in the window, floored.
    pub target_excess_ns: Vec<u64>,
    /// Per process of the mix: CPU time, the longest runnable wait and the longest
    /// wake-to-run time.
    pub cpu_ns: Vec<u64>,
    pub max_wait_ns: Vec<u64>,
    pub max_latency_ns: Vec<u64>,
    pub ejections: u32,
}

impl WindowMeasure {
    pub fn p99(&self) -> u64 {
        let mut sorted = self.target_excess_ns.clone();
        sorted.sort_unstable();
        percentile(&sorted, 99)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pair {
    pub baseline: WindowMeasure,
    pub candidate: WindowMeasure,
}

/// Why a candidate was reverted. Indices are into the mix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Guardrail {
        process: usize,
        wait_ns: u64,
    },
    Ejected,
    TooFewSamples,
    NotFaster,
    PairWorse {
        pair: usize,
    },
    BackgroundTotal {
        permille: u64,
    },
    BackgroundProcess {
        process: usize,
        permille: u64,
    },
    /// The trial could not run as planned: the baseline was ejected, or a switch
    /// to or from the candidate was refused.
    NotRun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    pub kept: bool,
    pub reason: Option<Reason>,
    pub baseline_p99_ns: u64,
    pub candidate_p99_ns: u64,
    /// Candidate background CPU as a share of the baseline's, in thousandths.
    pub background_permille: u64,
    /// The background process that lost the most CPU, and its share.
    pub worst_process: Option<(usize, u64)>,
}

impl Verdict {
    /// Candidate p99 over baseline p99, in thousandths.
    pub fn ratio_permille(&self) -> u64 {
        permille(self.candidate_p99_ns, self.baseline_p99_ns)
    }
}

pub fn permille(part: u64, whole: u64) -> u64 {
    if whole == 0 {
        return if part == 0 { 1000 } else { u64::MAX };
    }
    (u128::from(part) * 1000 / u128::from(whole)) as u64
}

fn pooled_p99(windows: impl Iterator<Item = WindowMeasure>) -> (u64, usize) {
    let mut all: Vec<u64> = windows.flat_map(|w| w.target_excess_ns).collect();
    all.sort_unstable();
    (percentile(&all, 99), all.len())
}

/// The rule, on measured windows.
pub fn judge(pairs: &[Pair], target: usize) -> Verdict {
    let (base_p99, base_n) = pooled_p99(pairs.iter().map(|p| p.baseline.clone()));
    let (cand_p99, cand_n) = pooled_p99(pairs.iter().map(|p| p.candidate.clone()));
    let processes = pairs.first().map_or(0, |p| p.baseline.cpu_ns.len());
    let sum = |side: fn(&Pair) -> &WindowMeasure, index: usize| -> u64 {
        pairs.iter().map(|p| side(p).cpu_ns[index]).sum()
    };
    let background = (0..processes).filter(|&i| i != target);
    let base_total: u64 = background.clone().map(|i| sum(|p| &p.baseline, i)).sum();
    let cand_total: u64 = background.clone().map(|i| sum(|p| &p.candidate, i)).sum();
    let base_time: u64 = pairs.iter().map(|p| p.baseline.to - p.baseline.from).sum();
    let worst_process = background
        .clone()
        .filter(|&i| sum(|p| &p.baseline, i) * 100 >= base_time)
        .map(|i| {
            (
                i,
                permille(sum(|p| &p.candidate, i), sum(|p| &p.baseline, i)),
            )
        })
        .min_by_key(|&(i, share)| (share, i));
    let mut verdict = Verdict {
        kept: false,
        reason: None,
        baseline_p99_ns: base_p99,
        candidate_p99_ns: cand_p99,
        background_permille: permille(cand_total, base_total),
        worst_process,
    };
    let guardrail = pairs.iter().find_map(|p| {
        let worst = (0..processes).max_by_key(|&i| (p.candidate.max_wait_ns[i], usize::MAX - i))?;
        let wait_ns = p.candidate.max_wait_ns[worst];
        (wait_ns > GUARDRAIL_WAIT_NS).then_some(Reason::Guardrail {
            process: worst,
            wait_ns,
        })
    });
    let worse = pairs
        .iter()
        .position(|p| p.candidate.p99() > p.baseline.p99());
    verdict.reason = if let Some(reason) = guardrail {
        Some(reason)
    } else if pairs.iter().any(|p| p.candidate.ejections > 0) {
        Some(Reason::Ejected)
    } else if pairs.is_empty() || base_n.min(cand_n) < MIN_SAMPLES {
        Some(Reason::TooFewSamples)
    } else if cand_p99 * 10 > base_p99 * 8 {
        Some(Reason::NotFaster)
    } else if let Some(pair) = worse {
        Some(Reason::PairWorse { pair })
    } else if cand_total * 10 < base_total * 9 {
        Some(Reason::BackgroundTotal {
            permille: verdict.background_permille,
        })
    } else if let Some((process, share)) = worst_process.filter(|&(_, share)| share < 800) {
        Some(Reason::BackgroundProcess {
            process,
            permille: share,
        })
    } else {
        None
    };
    verdict.kept = verdict.reason.is_none();
    verdict
}

/// The target's events in a run of the target alone: what its latency would be
/// with nothing else on the CPU.
pub struct Unloaded {
    events: Vec<EventRecord>,
}

impl Unloaded {
    pub fn run(mix: &Mix, seed: u64, plan: &TrialPlan) -> Self {
        let mut sim = Sim::alone(mix.procs, mix.target, seed, Box::new(Interactive::new()));
        sim.run_until(plan.total_ns());
        Self {
            events: sim.events(mix.target).to_vec(),
        }
    }

    /// The latency of the event scheduled at `at` when the target ran alone. The
    /// streams make every loaded event have a partner; one without is a bug.
    fn latency(&self, at: u64) -> u64 {
        let found = self.events.binary_search_by_key(&at, |event| event.at);
        debug_assert!(
            found.is_ok(),
            "an event at {at} ns has no partner in the run alone"
        );
        found.map_or(0, |i| self.events[i].latency_ns)
    }
}

/// A finished trial: its windows and the verdict on them.
pub struct Trial {
    pub pairs: Vec<Pair>,
    pub verdict: Verdict,
}

fn classes_table(sim: &Sim, mix: &Mix, choice: &Choice) -> [Class; MAX_THREADS] {
    let mut table = [Class::Normal; MAX_THREADS];
    for index in (0..mix.procs.len()).filter(|&i| choice.labelled(i)) {
        if let Some(tid) = sim.tid(index) {
            table[tid] = Class::Background;
        }
    }
    table
}

/// One window from now. The guardrail's waits and ejections count from its start;
/// CPU and latency only after the discard.
fn window(sim: &mut Sim, mix: &Mix, plan: &TrialPlan) -> WindowMeasure {
    let from = sim.now() + DISCARD_NS;
    let to = sim.now() + plan.window_ns;
    sim.core_mut().begin_window();
    let ejections = sim.core().ejections();
    sim.run_until(from);
    let start = sim.core().snapshot(from);
    sim.run_until(to);
    let measured = sim.core().snapshot(to).since(&start);
    let per = |field: fn(&crate::stats::ThreadWindow) -> u64| -> Vec<u64> {
        (0..mix.procs.len())
            .map(|i| sim.tid(i).map_or(0, |tid| field(&measured.threads[tid])))
            .collect()
    };
    WindowMeasure {
        from,
        to,
        target_excess_ns: Vec::new(),
        cpu_ns: per(|t| t.cpu_ns),
        max_wait_ns: per(|t| t.max_wait_ns),
        max_latency_ns: per(|t| t.max_latency_ns),
        ejections: sim.core().ejections() - ejections,
    }
}

/// Runs `candidate` against `baseline` on `mix` and judges it. The baseline is
/// installed first; the candidate is staged and flipped in and out at window
/// boundaries through the Core's allocation-free standby, as the kernel will.
pub fn run_trial(
    mix: &Mix,
    seed: u64,
    baseline: &Choice,
    candidate: &Choice,
    plan: &TrialPlan,
    unloaded: &Unloaded,
) -> Result<Trial, SpecError> {
    let mut sim = Sim::new(mix.procs, seed, baseline.spec.build()?);
    let installed = classes_table(&sim, mix, baseline);
    for (tid, class) in installed.into_iter().enumerate() {
        sim.core_mut().set_class(tid, class);
    }
    sim.run_until(plan.warmup_ns);
    let staged = classes_table(&sim, mix, candidate);
    let mut ran = sim
        .core_mut()
        .stage(candidate.spec.build()?, staged)
        .is_ok();
    let mut pairs = Vec::new();
    for _ in 0..plan.pairs {
        let baseline = window(&mut sim, mix, plan);
        let now = sim.now();
        // A refused switch (the baseline ejected) means the candidate never ran.
        ran &= sim.core().ejections() == 0 && sim.core_mut().flip(now);
        let candidate = window(&mut sim, mix, plan);
        let now = sim.now();
        ran &= sim.core_mut().revert(now);
        pairs.push(Pair {
            baseline,
            candidate,
        });
    }
    sim.run_until(sim.now() + plan.tail_ns);
    let end = sim.now();
    let finished = sim.events(mix.target).iter().copied();
    let unfinished = sim.pending(mix.target).map(|at| EventRecord {
        at,
        latency_ns: end - at,
    });
    for event in finished.chain(unfinished) {
        let excess = event.latency_ns.saturating_sub(unloaded.latency(event.at));
        let floored = excess.max(LATENCY_FLOOR_NS);
        for w in pairs
            .iter_mut()
            .flat_map(|p| [&mut p.baseline, &mut p.candidate])
        {
            if (w.from..w.to).contains(&event.at) {
                w.target_excess_ns.push(floored);
            }
        }
    }
    let mut verdict = judge(&pairs, mix.target);
    if !ran {
        verdict.kept = false;
        verdict.reason = Some(Reason::NotRun);
    }
    Ok(Trial { pairs, verdict })
}

/// Every distinct candidate for `mix`: the three label-free policies, then
/// `classes` with each preset and each set of labels on processes that wake
/// (a label on a hog changes nothing, since a hog never wakes).
pub fn candidates(mix: &Mix) -> Vec<Choice> {
    let mut all: Vec<Choice> = [
        PolicyKind::Interactive,
        PolicyKind::RoundRobin,
        PolicyKind::Responsive,
    ]
    .into_iter()
    .map(|kind| Choice::new(PolicySpec::new(kind, Preset::Gentle), 0))
    .collect();
    let labelable: Vec<usize> = mix.labelable().collect();
    for preset in Preset::ALL {
        for subset in 0u32..1 << labelable.len() {
            let mask = labelable
                .iter()
                .enumerate()
                .filter(|&(bit, _)| subset & (1 << bit) != 0)
                .fold(0, |mask, (_, &index)| mask | 1 << index);
            all.push(Choice::new(
                PolicySpec::new(PolicyKind::Classes, preset),
                mask,
            ));
        }
    }
    all
}

/// How one candidate did over several seeds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Score {
    pub choice: Choice,
    pub verdicts: Vec<Verdict>,
}

impl Score {
    pub fn kept_everywhere(&self) -> bool {
        self.verdicts.iter().all(|v| v.kept)
    }

    pub fn kept(&self) -> usize {
        self.verdicts.iter().filter(|v| v.kept).count()
    }

    /// The candidate's p99 over the baseline's, in thousandths, averaged.
    pub fn mean_ratio_permille(&self) -> u64 {
        let n = self.verdicts.len().max(1) as u64;
        let sum = self
            .verdicts
            .iter()
            .fold(0u64, |sum, v| sum.saturating_add(v.ratio_permille()));
        sum / n
    }
}

/// Trials of each choice against Interactive on each seed. The run of the
/// target alone is shared by every choice of a seed.
pub fn evaluate(
    mix: &Mix,
    seeds: &[u64],
    choices: &[Choice],
    plan: &TrialPlan,
) -> Result<Vec<Score>, SpecError> {
    let unloaded: Vec<Unloaded> = seeds.iter().map(|&s| Unloaded::run(mix, s, plan)).collect();
    choices
        .iter()
        .map(|choice| {
            let verdicts = seeds
                .iter()
                .zip(&unloaded)
                .map(|(&seed, alone)| {
                    run_trial(mix, seed, &Choice::DEFAULT, choice, plan, alone).map(|t| t.verdict)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Score {
                choice: *choice,
                verdicts,
            })
        })
        .collect()
}

/// The best choice found by trying them all: kept on every seed, with the lowest
/// mean ratio; the earliest in `candidates` order on a tie. `None` if nothing is
/// kept everywhere, which means keep.
pub fn best(scores: &[Score]) -> Option<&Score> {
    scores
        .iter()
        .filter(|score| score.kept_everywhere())
        .min_by_key(|score| score.mean_ratio_permille())
}

#[cfg(test)]
mod tests;
