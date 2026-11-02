//! `test=desk-perf`'s verdict lines (P3 close), shared by `desktop`, which
//! writes them, and the kernel, which judges them. A figure over its budget
//! adds a line `OVER BUDGET <figure> <value> ms (budget <p99|max> <b> ms)`.
//!
//! Shared runners are noisy, so the gate is best of two: a scene with a miss
//! is measured once more in the same boot, and only a figure over its budget
//! in both runs fails the case ([`still_over`]). The budgets stay as they are.

use alloc::vec::Vec;

/// Starts the line after a figure over its budget.
pub const OVER_BUDGET: &str = "OVER BUDGET ";

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

/// The figures over budget in both runs of one scene, by name.
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
}
