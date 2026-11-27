//! `test=desk-perf`'s verdict lines (P3 close), shared by `desktop`, which
//! writes them, and the kernel, which judges them. A figure over its budget
//! adds a line `OVER BUDGET <figure> <value> ms (budget <p99|max> <b> ms)`.
//!
//! Shared runners are noisy, so the gate is best of three: a scene with a miss
//! is measured again in the same boot, a figure over in two runs a third time,
//! and only a figure over its budget in every run fails the case
//! ([`still_over`], applied to each pair).
//!
//! Each budget is set for a runner of baseline speed. Under KVM a figure is a
//! miss only when it is over its budget times the runner's measured slowness,
//! the reference median over [`REFERENCE_BASELINE_NS`], never less than one
//! ([`misses_scaled`]): a runner 1.85 times the baseline gets an 8 ms budget
//! of 14.8 ms. The scale is capped at the slow factor ([`SCALE_CAP_NS`]).
//! Before a scene's second run the reference is measured again, and both runs
//! are judged by the larger of the two medians ([`judging_reference`]), so
//! a runner that slows within the boot is seen. `desktop` still writes its
//! `OVER BUDGET` lines against the bare budgets, so the logs show both. After
//! each such line it logs the figure's slowest frames, four to a line:
//! `desk-perf: <name>: the 8 slowest frames (1/2): #<n> <ms>, ...`.
//!
//! A runner can be slow for a whole boot, not only noisy, and then no budget
//! can be met. So under KVM the kernel first has `desktop` time a fixed
//! reference work ([`reference_work`]), which shares no code with the desk, so
//! a slower desk never makes the runner look slow. A median over
//! [`SLOW_FACTOR_TENTHS`] tenths of [`REFERENCE_BASELINE_NS`] ([`too_slow`]) means the
//! figures are reported and not gated.

use alloc::vec::Vec;

/// Starts the line after a figure over its budget.
pub const OVER_BUDGET: &str = "OVER BUDGET ";

/// Starts `desktop`'s reply to `perf reference`: `reference p50 <ns> ns: ...`.
pub const REFERENCE: &str = "reference p50 ";
/// The reference work's median on GitHub's runners under KVM, in ns, as
/// recorded in `docs/decisions.md`.
pub const REFERENCE_BASELINE_NS: u64 = 16_000_000;
/// A median over this many tenths of the baseline, 3.5 times it (56 ms), is a
/// runner too slow to judge; `docs/decisions.md` says why.
pub const SLOW_FACTOR_TENTHS: u64 = 35;
/// The reference work's buffer, in 32-bit words: 2 MiB.
pub const REFERENCE_WORDS: usize = 512 * 1024;
/// How many times one reference work fills and reads its buffer.
const REFERENCE_PASSES: u32 = 4;

/// The reference work: [`REFERENCE_PASSES`] times, `words` filled from a
/// xorshift seeded by `seed` and the pass, then folded into the result. Plain
/// integer work and memory traffic, as a composite is, and fixed: it changes
/// only if this function does, which moves the baseline.
pub fn reference_work(words: &mut [u32], seed: u32) -> u32 {
    let mut folded = seed;
    for pass in 0..REFERENCE_PASSES {
        let mut x = (seed ^ pass.wrapping_mul(0x9E37_79B9)) | 1;
        for word in words.iter_mut() {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *word = x;
        }
        for &word in words.iter() {
            folded = folded.rotate_left(5) ^ word;
        }
    }
    folded
}

/// The median in `desktop`'s reply to `perf reference`, in ns.
pub fn reference_ns(reply: &str) -> Option<u64> {
    let rest = reply.lines().find_map(|l| l.strip_prefix(REFERENCE))?;
    rest.split_once(" ns")?.0.parse().ok()
}

/// Whether a reference median of `p50_ns` is a runner too slow to judge.
pub fn too_slow(p50_ns: u64) -> bool {
    p50_ns.saturating_mul(10) > REFERENCE_BASELINE_NS.saturating_mul(SLOW_FACTOR_TENTHS)
}

/// The figures over budget in `reply`, by name: `(c) text page 160x45 warm`
/// for `OVER BUDGET (c) text page 160x45 warm 5.14 ms (budget max 4.00 ms)`.
pub fn misses(reply: &str) -> Vec<&str> {
    reply
        .lines()
        .filter_map(|l| l.strip_prefix(OVER_BUDGET))
        .filter_map(|l| l.split_once(" ms (budget")?.0.rsplit_once(' '))
        .map(|(name, _)| name)
        .collect()
}

/// A miss line's figure, value and budget, the last two in hundredths of a
/// ms: `("(c) text page 160x45 warm", 514, 400)` for
/// `OVER BUDGET (c) text page 160x45 warm 5.14 ms (budget max 4.00 ms)`.
fn over_line(line: &str) -> Option<(&str, u64, u64)> {
    let (figure, budget) = line.strip_prefix(OVER_BUDGET)?.split_once(" ms (budget ")?;
    let (name, value) = figure.rsplit_once(' ')?;
    let budget = budget.strip_suffix(" ms)")?.rsplit_once(' ')?.1;
    Some((name, hundredths(value)?, hundredths(budget)?))
}

/// `"5.14"` as 514; at most two decimals.
fn hundredths(text: &str) -> Option<u64> {
    let (whole, frac) = text.split_once('.').unwrap_or((text, ""));
    if frac.len() > 2 || whole.is_empty() {
        return None;
    }
    let whole: u64 = whole.parse().ok()?;
    let frac: u64 = match frac.len() {
        0 => 0,
        1 => frac.parse::<u64>().ok()? * 10,
        _ => frac.parse().ok()?,
    };
    whole.checked_mul(100)?.checked_add(frac)
}

/// The reference median the scale stops at: the slow factor, 3.5 times the
/// baseline (56 ms).
pub const SCALE_CAP_NS: u64 = REFERENCE_BASELINE_NS * SLOW_FACTOR_TENTHS / 10;

/// A budget in hundredths of a ms, scaled for a runner whose reference median
/// is `reference_ns`: times `reference_ns` over the baseline, never less than
/// the budget itself and never more than [`SCALE_CAP_NS`] allows.
pub fn scaled_budget(budget: u64, reference_ns: u64) -> u64 {
    let ratio = reference_ns.clamp(REFERENCE_BASELINE_NS, SCALE_CAP_NS);
    budget.saturating_mul(ratio) / REFERENCE_BASELINE_NS
}

/// The figure judged against the composite as well as the reference: (g), the
/// full composite with Start and the pane open.
pub const COMPOSITE_FIGURE: &str = "(g) ";
/// The figure whose p50 stands for the composite's cost in this boot: (a).
pub const COMPOSITE_REFERENCE: &str = "(a) full composite";
/// (g)'s p99 may reach this many hundredths of (a)'s p50 from the same boot.
/// The reference work undersells the composite on some CPU classes (an EPYC
/// 9V45 or a Xeon 8573C runs (a) about 20% slower than the reference says),
/// while (g) over (a)'s p50 stays within 1.15 to 1.43 on every class main has
/// run on. At 1.40 no main run's verdict changes, nor that of the same run
/// 15%, 20% or 25% slower in (g)
/// (`docs/decisions/rows/2026-11-23-desk-perf-scaled-budgets.md`).
pub const COMPOSITE_TIMES_HUNDREDTHS: u64 = 140;

/// (g)'s budget in hundredths of a ms: the larger of its [`scaled_budget`] and
/// [`COMPOSITE_TIMES_HUNDREDTHS`] of `composite_p50`, (a)'s p50 from the same
/// boot, that second term never past the slow factor's scale of the budget.
/// Only ever at least the scaled budget, so only (g)'s judging changes.
pub fn composite_budget(budget: u64, reference_ns: u64, composite_p50: u64) -> u64 {
    let composite = composite_p50.saturating_mul(COMPOSITE_TIMES_HUNDREDTHS) / 100;
    let cap = budget.saturating_mul(SLOW_FACTOR_TENTHS) / 10;
    scaled_budget(budget, reference_ns).max(composite.min(cap))
}

/// The p50 of the figure in `reply` whose name starts with `prefix`, in
/// hundredths of a ms: 1864 for `(a) full composite at 2560x1440: n=300 p50
/// 18.64 p95 ...`.
pub fn p50(reply: &str, prefix: &str) -> Option<u64> {
    let line = reply.lines().find(|l| l.starts_with(prefix))?;
    let rest = line.split_once(": n=")?.1;
    let mut words = rest.split(' ');
    words.find(|&w| w == "p50")?;
    hundredths(words.next()?)
}

/// The reference median a scene's two runs are judged by: the larger of the
/// boot's first measurement and the one taken just before the second run.
/// [`scaled_budget`] caps it.
pub fn judging_reference(first_ns: u64, again_ns: u64) -> u64 {
    first_ns.max(again_ns)
}

/// One figure line of `desktop`'s reply:
/// `name: n=.. p50 .. p95 .. p99 .. max .. ms, budget <p99|max> .. ms: ...`.
#[derive(Debug, PartialEq, Eq)]
pub struct Figure<'a> {
    pub name: &'a str,
    /// `p99` or `max`: the statistic the budget is for.
    pub label: &'a str,
    /// That statistic and the budget, in hundredths of a ms.
    pub value: u64,
    pub budget: u64,
}

/// Every judged figure in `reply`, in order; lines that do not parse are left
/// out.
pub fn figures(reply: &str) -> Vec<Figure<'_>> {
    reply.lines().filter_map(figure_line).collect()
}

fn figure_line(line: &str) -> Option<Figure<'_>> {
    let (name, rest) = line.split_once(": n=")?;
    let (stats, budget) = rest.split_once(" ms, budget ")?;
    let (label, budget) = budget.split_once(' ')?;
    let budget = budget.split_once(" ms")?.0;
    let mut words = stats.split(' ');
    words.find(|&w| w == label)?;
    Some(Figure {
        name,
        label,
        value: hundredths(words.next()?)?,
        budget: hundredths(budget)?,
    })
}

/// `value` (hundredths of a ms) divided by the runner's ratio to the baseline,
/// in hundredths of a ms: what the figure would be on a runner of baseline
/// speed if the reference predicted it exactly. Not clamped, for the logs.
pub fn per_baseline(value: u64, reference_ns: u64) -> u64 {
    value.saturating_mul(REFERENCE_BASELINE_NS) / reference_ns.max(1)
}

/// `reference_ns` over the baseline, in hundredths: `185` for 29.6 ms.
pub fn ratio_hundredths(reference_ns: u64) -> u64 {
    reference_ns.saturating_mul(100) / REFERENCE_BASELINE_NS
}

/// Hundredths of a ms (or of a ratio) as text: `"14.80"` for 1480.
pub fn hundredths_text(value: u64) -> alloc::string::String {
    alloc::format!("{}.{:02}", value / 100, value % 100)
}

/// The figures in `reply` over their budget scaled for a runner whose
/// reference median is `reference_ns` ([`scaled_budget`]), by name. A value
/// that reaches the scaled budget is a miss, so on a runner at or under the
/// baseline these are exactly [`misses`]. A miss line whose value or budget
/// does not parse is kept, so a line [`misses`] can name never passes a
/// figure.
pub fn misses_scaled(reply: &str, reference_ns: u64) -> Vec<&str> {
    misses_judged(reply, reference_ns, None)
}

/// [`misses_scaled`], with (g) ([`COMPOSITE_FIGURE`]) judged against its
/// [`composite_budget`] when `composite_p50`, (a)'s p50 from this boot, is
/// known.
pub fn misses_judged(reply: &str, reference_ns: u64, composite_p50: Option<u64>) -> Vec<&str> {
    reply
        .lines()
        .filter(|l| l.starts_with(OVER_BUDGET))
        .filter_map(|l| match over_line(l) {
            Some((name, value, budget)) => {
                (value >= judged_budget(name, budget, reference_ns, composite_p50)).then_some(name)
            }
            None => misses(l).into_iter().next(),
        })
        .collect()
}

/// The budget figure `name` is judged against: [`composite_budget`] for (g)
/// when (a)'s p50 is known, else [`scaled_budget`].
pub fn judged_budget(
    name: &str,
    budget: u64,
    reference_ns: u64,
    composite_p50: Option<u64>,
) -> u64 {
    match composite_p50 {
        Some(p50) if name.starts_with(COMPOSITE_FIGURE) => {
            composite_budget(budget, reference_ns, p50)
        }
        _ => scaled_budget(budget, reference_ns),
    }
}

/// How many times the baseline a reference median of `p50_ns` is, to a
/// tenth: `2.8` for 44.56 ms against 16.
pub fn times_baseline(p50_ns: u64) -> alloc::string::String {
    let tenths = p50_ns.saturating_mul(10) / REFERENCE_BASELINE_NS;
    alloc::format!("{}.{}", tenths / 10, tenths % 10)
}

/// The figures over budget in both of two runs of one scene, by name.
pub fn still_over<'a>(first: &[&'a str], second: &[&str]) -> Vec<&'a str> {
    first
        .iter()
        .copied()
        .filter(|name| second.contains(name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WARM: &str = "(c) text page 160x45 warm: n=300 p50 2.10 p95 2.40 p99 4.57 max 5.14 ms, budget max 4.00 ms: OVER\n\
        OVER BUDGET (c) text page 160x45 warm 5.14 ms (budget max 4.00 ms)\n\
        (c) text page 160x45 cold: n=10 p50 9.00 p95 9.90 p99 9.90 max 9.90 ms, budget max 30.00 ms: within";

    #[test]
    fn a_miss_is_named_by_its_figure_without_its_value() {
        assert_eq!(misses(WARM), ["(c) text page 160x45 warm"]);
        assert!(misses("(a) full composite at 1920x1080: ... within").is_empty());
        // A line that only looks like one names nothing.
        assert!(misses("OVER BUDGET garbled").is_empty());
    }

    #[test]
    fn only_a_figure_over_in_both_runs_fails() {
        let first = ["(c) text page 160x45 warm", "(b) drag flush"];
        assert_eq!(still_over(&first, &["(b) drag flush"]), ["(b) drag flush"]);
        assert!(still_over(&first, &[]).is_empty());
        assert!(still_over(&[], &["(b) drag flush"]).is_empty());
    }

    #[test]
    fn the_reference_work_is_fixed_by_its_seed_and_fills_its_buffer() {
        let mut a = alloc::vec![0u32; 4096];
        let mut b = alloc::vec![0u32; 4096];
        assert_eq!(reference_work(&mut a, 7), reference_work(&mut b, 7));
        assert_eq!(a, b);
        assert!(a.iter().all(|&w| w != 0));
        assert_ne!(reference_work(&mut a, 7), reference_work(&mut b, 8));
    }

    #[test]
    fn the_reference_median_is_read_from_desktops_reply() {
        let reply = "reference p50 1234567 ns: n=21 p50 1.23 max 1.40 ms";
        assert_eq!(reference_ns(reply), Some(1_234_567));
        assert_eq!(reference_ns("(a) full composite: ..."), None);
        assert_eq!(reference_ns("reference p50 fast ns"), None);
    }

    #[test]
    fn a_runner_is_too_slow_only_past_three_and_a_half_times_the_baseline() {
        assert!(!too_slow(REFERENCE_BASELINE_NS));
        assert!(!too_slow(56_000_000));
        assert!(too_slow(56_000_001));
        // The slowest in-budget runner seen, 48.09 ms, is gated.
        assert!(!too_slow(48_087_560));
        assert!(!too_slow(0));
    }

    #[test]
    fn a_miss_line_gives_its_value_and_budget_in_hundredths() {
        let line = "OVER BUDGET (c) text page 160x45 warm 5.14 ms (budget max 4.00 ms)";
        assert_eq!(
            over_line(line),
            Some(("(c) text page 160x45 warm", 514, 400))
        );
        assert_eq!(hundredths("16.7"), Some(1670));
        assert_eq!(hundredths("8"), Some(800));
        assert_eq!(hundredths("1.234"), None);
        assert_eq!(hundredths(".5"), None);
        assert_eq!(over_line("OVER BUDGET garbled"), None);
    }

    #[test]
    fn budgets_scale_with_the_runner_but_never_shrink() {
        assert_eq!(scaled_budget(800, REFERENCE_BASELINE_NS), 800);
        assert_eq!(scaled_budget(800, REFERENCE_BASELINE_NS / 2), 800);
        assert_eq!(scaled_budget(800, 0), 800);
        // #1261's runner: 29.60 ms, 1.85 times the baseline.
        assert_eq!(scaled_budget(800, 29_603_970), 1480);
        // The slowest gated runner, 3.5 times: 28 ms.
        assert_eq!(scaled_budget(800, 56_000_000), 2800);
    }

    #[test]
    fn the_scale_stops_at_the_slow_factor() {
        assert_eq!(SCALE_CAP_NS, 56_000_000);
        assert_eq!(scaled_budget(800, 80_000_000), 2800);
        assert_eq!(scaled_budget(1670, u64::MAX), 5845);
    }

    #[test]
    fn both_runs_are_judged_by_the_larger_reference() {
        assert_eq!(judging_reference(20_000_000, 30_000_000), 30_000_000);
        assert_eq!(judging_reference(30_000_000, 20_000_000), 30_000_000);
        // A runner that slowed past the slow factor in the boot is judged at
        // the cap, not ungated.
        let judged = judging_reference(29_603_970, 70_000_000);
        assert_eq!(scaled_budget(800, judged), 2800);
        // #1261's (g) at the first ratio, 1.85, against a remeasured 2.2.
        let g = "OVER BUDGET (g) full composite with Start and the pane open at 1920x1080 15.20 ms (budget p99 8.00 ms)";
        assert_eq!(misses_scaled(g, 29_603_970).len(), 1);
        let again = judging_reference(29_603_970, 35_200_000);
        assert!(misses_scaled(g, again).is_empty());
        // A faster second measurement never tightens the budget.
        assert_eq!(judging_reference(29_603_970, 16_000_000), 29_603_970);
    }

    #[test]
    fn every_figure_line_gives_its_judged_value_and_budget() {
        let found = figures(WARM);
        assert_eq!(
            found,
            [
                Figure {
                    name: "(c) text page 160x45 warm",
                    label: "max",
                    value: 514,
                    budget: 400
                },
                Figure {
                    name: "(c) text page 160x45 cold",
                    label: "max",
                    value: 990,
                    budget: 3000
                },
            ]
        );
        let g = "(g) full composite with Start and the pane open at 1920x1080: n=300 p50 9.80 p95 11.86 p99 13.47 max 13.83 ms, budget p99 8.00 ms: OVER";
        assert_eq!(figures(g)[0].value, 1347);
        assert_eq!(figures(g)[0].label, "p99");
        assert!(figures("(b) drag render: no samples").is_empty());
    }

    #[test]
    fn a_figure_is_told_per_baseline() {
        // #1261's (g): 13.47 ms at 1.85 is 7.28 ms per baseline.
        assert_eq!(ratio_hundredths(29_603_970), 185);
        assert_eq!(per_baseline(1347, 29_603_970), 728);
        assert_eq!(hundredths_text(728), "7.28");
        assert_eq!(hundredths_text(5), "0.05");
        assert_eq!(per_baseline(800, 0), 800 * REFERENCE_BASELINE_NS);
    }

    /// The three failures of 2026-11-23 the scaling must pass, and what it
    /// still fails.
    #[test]
    fn a_figure_is_judged_against_its_scaled_budget() {
        let g = |value: &str| {
            alloc::format!(
                "OVER BUDGET (g) full composite with Start and the pane open at 1920x1080 {value} ms (budget p99 8.00 ms)"
            )
        };
        let f = "OVER BUDGET (f) drag under load, render+flush 20.72 ms (budget p99 16.70 ms)";
        // #1119: (f) 20.72 ms on a runner 1.34 times the baseline (22.40).
        assert!(misses_scaled(f, 21_467_770).is_empty());
        // #1141: (g) 9.26 ms at 2.52 times (20.17).
        assert!(misses_scaled(&g("9.26"), 40_347_570).is_empty());
        // #1261: (g) 13.65 ms at 1.85 times (14.80).
        assert!(misses_scaled(&g("13.65"), 29_603_970).is_empty());
        // Main's runner, 1.29 times: (g) 10.62 ms, half again its 6.96 ms, fails.
        assert_eq!(
            misses_scaled(&g("10.62"), 20_590_190),
            ["(g) full composite with Start and the pane open at 1920x1080"]
        );
        // At or under the baseline the bare budget holds, as `desktop` judged.
        assert_eq!(misses_scaled(&g("8.01"), 15_000_000).len(), 1);
        assert_eq!(misses_scaled(WARM, REFERENCE_BASELINE_NS), misses(WARM));
        // A garbled miss line still counts.
        let garbled = "OVER BUDGET (b) drag flush 5.00 ms (budget soon)";
        assert_eq!(misses_scaled(garbled, 40_000_000), ["(b) drag flush"]);
    }

    /// `desk-perf-1440`'s (g) on #1280's runs (2560x1440, budget 14.40 ms): 15.36
    /// and 15.50 ms at 1.34 times the baseline (the first two runs) and 15.10 and
    /// 15.13 ms at 1.33 times (the last two) failed the bare budget; scaled they
    /// pass (19.2 ms), and half as much again still fails.
    #[test]
    fn the_1440_composite_with_start_and_the_pane_is_judged_scaled() {
        let g = |value: &str| {
            alloc::format!(
                "OVER BUDGET (g) full composite with Start and the pane open at 2560x1440 {value} ms (budget p99 14.40 ms)"
            )
        };
        assert!(misses_scaled(&g("15.36"), 21_493_210).is_empty());
        assert!(misses_scaled(&g("15.50"), 21_493_210).is_empty());
        assert!(misses_scaled(&g("15.10"), 21_318_740).is_empty());
        assert!(misses_scaled(&g("15.13"), 21_318_740).is_empty());
        assert_eq!(misses_scaled(&g("22.00"), 21_318_740).len(), 1);
        // On a runner at the baseline the bare budget holds.
        assert_eq!(misses_scaled(&g("15.10"), REFERENCE_BASELINE_NS).len(), 1);
    }

    fn g1440(value: &str) -> alloc::string::String {
        alloc::format!(
            "OVER BUDGET (g) full composite with Start and the pane open at 2560x1440 {value} ms (budget p99 14.40 ms)"
        )
    }

    #[test]
    fn the_composites_p50_is_read_from_its_figure_line() {
        let a = "(a) full composite at 2560x1440: n=300 p50 18.64 p95 19.68 p99 20.01 max 20.40 ms, budget p99 14.40 ms: OVER";
        let reply = alloc::format!(
            "{a}\n(b) drag render: n=10 p50 1.00 p95 1.00 p99 1.00 max 1.00 ms, budget p99 4.00 ms: within"
        );
        assert_eq!(p50(&reply, COMPOSITE_REFERENCE), Some(1864));
        assert_eq!(
            p50("(b) drag render: no samples", COMPOSITE_REFERENCE),
            None
        );
    }

    /// (g)'s budget is the larger of the reference's and 1.40 times (a)'s p50,
    /// the second never past the slow factor; other figures are as before.
    #[test]
    fn the_composite_budget_is_the_larger_of_the_two() {
        // Main's 8573C run (job 139591500596): 1.73x, (a) p50 18.64 ms.
        assert_eq!(scaled_budget(1440, 27_689_930), 2492);
        assert_eq!(composite_budget(1440, 27_689_930, 1864), 2609);
        // A 7763 at 2.51x: the reference's 36.13 ms is the larger.
        assert_eq!(composite_budget(1440, 40_147_810, 1848), 3613);
        // A fast runner, under the baseline: never under the bare budget.
        assert_eq!(composite_budget(1440, 15_000_000, 900), 1440);
        // A stalled (a) cannot lift (g) past 3.5 times its budget.
        assert_eq!(composite_budget(1440, 16_000_000, 10_000), 5040);
        assert_eq!(
            judged_budget("(f) drag under load", 1670, 21_467_770, Some(9999)),
            2240
        );
        assert_eq!(judged_budget("(g) x", 1440, 27_689_930, None), 2492);
    }

    /// The three PR runs of 2026-11-27 that failed (g) with no desk change
    /// pass, judged against (a) from their own boot; best of three, each run
    /// at the reference it was judged by.
    #[test]
    fn the_runners_that_undersold_the_composite_pass() {
        let g = "(g) full composite with Start and the pane open at 2560x1440";
        let over = |value: &str, reference: u64, a: u64| {
            misses_judged(&g1440(value), reference, Some(a)).contains(&g)
        };
        // Job 140323621715, EPYC 9V45 at 1.18x, (a) p50 13.28 ms.
        assert!(!misses_scaled(&g1440("18.56"), 18_820_500).is_empty());
        assert!(!over("17.40", 19_316_140, 1328));
        // Job 140323631043, Xeon 6975P-C at 1.31x, (a) p50 14.61 ms.
        assert!(!misses_scaled(&g1440("19.00"), 21_093_540).is_empty());
        assert!(!over("19.00", 21_093_540, 1461));
        // Job 140284960282, Xeon 8573C at 1.68x, (a) p50 19.62 ms.
        assert!(!misses_scaled(&g1440("25.57"), 26_841_400).is_empty());
        assert!(!over("25.57", 26_841_400, 1962));
    }

    /// What it still catches: on main's 8573C run (g) at 24.16 ms passes, and
    /// 15% slower, 27.78 ms, fails; on main's usual 6975P-C (job 140190314306,
    /// 1.35x, (a) p50 14.17 ms, (g) 17.55 ms) 15% slower, 20.18 ms, fails.
    #[test]
    fn a_slower_composite_with_start_still_fails() {
        let g = "(g) full composite with Start and the pane open at 2560x1440";
        assert!(misses_judged(&g1440("24.16"), 27_689_930, Some(1864)).is_empty());
        assert_eq!(misses_judged(&g1440("27.78"), 27_689_930, Some(1864)), [g]);
        assert!(misses_judged(&g1440("17.55"), 21_615_060, Some(1417)).is_empty());
        assert_eq!(misses_judged(&g1440("20.18"), 21_615_060, Some(1417)), [g]);
    }

    #[test]
    fn slowness_is_told_in_tenths_of_the_baseline() {
        assert_eq!(times_baseline(REFERENCE_BASELINE_NS), "1.0");
        assert_eq!(times_baseline(REFERENCE_BASELINE_NS * 28 / 10), "2.8");
        assert_eq!(times_baseline(REFERENCE_BASELINE_NS * 2 + 1), "2.0");
    }
}
