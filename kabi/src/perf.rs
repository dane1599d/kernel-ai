//! `test=desk-perf`'s verdict lines (P3 close), shared by `desktop`, which
//! writes them, and the kernel, which judges them. A figure over its budget
//! adds a line `OVER BUDGET <figure> <value> ms (budget <p99|max> <b> ms)`.
//!
//! Shared runners are noisy, so the gate is best of two: a scene with a miss
//! is measured once more in the same boot, and only a figure over its budget
//! in both runs fails the case ([`still_over`]). The budgets stay as they are.
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

/// How many times the baseline a reference median of `p50_ns` is, to a
/// tenth: `2.8` for 44.56 ms against 16.
pub fn times_baseline(p50_ns: u64) -> alloc::string::String {
    let tenths = p50_ns.saturating_mul(10) / REFERENCE_BASELINE_NS;
    alloc::format!("{}.{}", tenths / 10, tenths % 10)
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
    fn slowness_is_told_in_tenths_of_the_baseline() {
        assert_eq!(times_baseline(REFERENCE_BASELINE_NS), "1.0");
        assert_eq!(times_baseline(REFERENCE_BASELINE_NS * 28 / 10), "2.8");
        assert_eq!(times_baseline(REFERENCE_BASELINE_NS * 2 + 1), "2.0");
    }
}
