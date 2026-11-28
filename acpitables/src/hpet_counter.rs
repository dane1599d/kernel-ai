//! Reading the HPET's 64-bit main counter so a torn read is never returned.
//!
//! A device may serve a 64-bit read as two 32-bit reads that each sample the
//! counter: QEMU before 10.0 does. If the low half wraps between the two samples,
//! the low half from before the wrap meets the high half from after it, and the
//! value is 2^32 ticks ahead (about 43 s at QEMU's 100 MHz). Such a value is never
//! returned: a 64-bit read is kept only if it lies within [`WINDOW`] ticks of an
//! earlier good value, and otherwise the halves are read high, low, high until the
//! two high halves agree.
//! A 32-bit main counter (CAPABILITIES bit 13 clear) cannot tear but wraps every
//! 2^32 ticks (300 s at 14.318 MHz): [`extend32`] carries it into 64 bits.

/// How far a 64-bit read may lie from the last good value and still be kept:
/// half the distance of a torn read, and about 21 s at 100 MHz.
pub const WINDOW: u64 = 1 << 31;

/// Whether `ticks`, from one 64-bit read, lies within [`WINDOW`] of `last`, an
/// earlier good read (on any CPU, so possibly slightly later than `ticks`).
pub fn plausible(ticks: u64, last: u64) -> bool {
    ticks.wrapping_sub(last).wrapping_add(WINDOW) < 2 * WINDOW
}

/// The counter from its 32-bit halves: high, low, high, again while the two high
/// halves differ, so the low half belongs to the high half returned.
pub fn read_halves(mut low: impl FnMut() -> u32, mut high: impl FnMut() -> u32) -> u64 {
    loop {
        let first = high();
        let low = low();
        if high() == first {
            return u64::from(first) << 32 | u64::from(low);
        }
    }
}

/// The counter, given a 64-bit read, the two 32-bit halves and the last good
/// value. Returns the value and whether the halves had to be read.
pub fn read(
    whole: impl FnOnce() -> u64,
    low: impl FnMut() -> u32,
    high: impl FnMut() -> u32,
    last: u64,
) -> (u64, bool) {
    let ticks = whole();
    if plausible(ticks, last) {
        (ticks, false)
    } else {
        (read_halves(low, high), true)
    }
}

/// Whether the main counter is 64 bits wide: CAPABILITIES bit 13, COUNT_SIZE_CAP.
pub fn counter_is_64bit(capabilities: u64) -> bool {
    capabilities & (1 << 13) != 0
}

/// A 32-bit counter's 64-bit count: the value within 2^31 ticks of `last` (an
/// earlier count, maybe a little later from another CPU) whose low half is `low`.
/// Right while the counter is read at least every 2^31 ticks (150 s at 14.3 MHz).
pub fn extend32(low: u32, last: u64) -> u64 {
    let step = low.wrapping_sub(last as u32) as i32;
    last.wrapping_add_signed(i64::from(step))
}

/// How far a CPU's clock read from the boot CPU's TSC zero point may be from
/// its own HPET read for the CPU to share that zero point. Each zero point pairs
/// an HPET read with the TSC at about the moment the HPET was sampled, off by
/// part of a read's few microseconds under KVM, so the difference is that of two
/// such errors and either sign; a vCPU whose TSC started apart (VirtualBox
/// starts each when the vCPU first runs) is off by far more.
pub const SHARED_ZERO_NS: u64 = 50_000;

/// Whether a CPU shares the boot CPU's TSC zero point: `from_boot_zero`, its
/// clock read against that zero point at the TSC of its own zero pair, lies
/// within [`SHARED_ZERO_NS`] of `hpet`, that pair's HPET read, either side.
/// With one zero point every CPU reads the same time, so no CPU's clock sits
/// still behind another's.
pub fn shares_zero(from_boot_zero: u64, hpet: u64) -> bool {
    from_boot_zero.abs_diff(hpet) <= SHARED_ZERO_NS
}

/// The zero pair from samples of (TSC before, HPET, TSC after) one HPET read:
/// the sample whose read took the fewest TSC ticks, as (the TSC midway through
/// that read, the HPET value). An HPET read the host was slow to serve, or that
/// was preempted, would put a zero point microseconds off; the quickest of a few
/// is off by at most half of its own read. `None` with no samples.
pub fn best_pair(samples: impl IntoIterator<Item = (u64, u64, u64)>) -> Option<(u64, u64)> {
    samples
        .into_iter()
        .min_by_key(|&(before, _, after)| after.wrapping_sub(before))
        .map(|(before, hpet, after)| (before + after.wrapping_sub(before) / 2, hpet))
}

/// Whether a measured TSC rate is one to trust: within 1% of the rate the
/// hypervisor reports (CPUID 0x40000010), when it reports one, and within 0.5%
/// of `last`, the window before. With no reported rate, the second alone (and
/// the kernel's drift check after it) stands between a KVM boot and a wrong
/// rate, so it is tested here (#1628's review).
pub fn settled(rate: u64, last: u64, reported_hz: Option<u64>) -> bool {
    let steady = rate != 0 && rate.abs_diff(last) * 200 < last;
    steady && reported_hz.is_none_or(|hz| rate.abs_diff(hz) * 100 < hz)
}

/// How far the TSC clock moved from the HPET over one re-check, in parts per
/// million of the HPET's time: `tsc` and `hpet` are each clock's nanoseconds at
/// the start and the end of it. A TSC whose rate changed after boot (a host
/// without a constant TSC, a migration without TSC scaling) shows here.
pub fn recheck_ppm(tsc: (u64, u64), hpet: (u64, u64)) -> u64 {
    let tsc_span = tsc.1.saturating_sub(tsc.0);
    let hpet_span = hpet.1.saturating_sub(hpet.0).max(1);
    (u128::from(tsc_span.abs_diff(hpet_span)) * 1_000_000 / u128::from(hpet_span)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;

    /// A counter that advances `step` ticks at every 32-bit sample, as a device
    /// that splits 64-bit reads in two would, starting at `start`.
    struct Splitting {
        now: Cell<u64>,
        step: u64,
    }

    impl Splitting {
        fn new(start: u64, step: u64) -> Self {
            Self {
                now: Cell::new(start),
                step,
            }
        }

        fn sample(&self) -> u64 {
            let now = self.now.get();
            self.now.set(now + self.step);
            now
        }

        fn low(&self) -> u32 {
            self.sample() as u32
        }

        fn high(&self) -> u32 {
            (self.sample() >> 32) as u32
        }

        /// A 64-bit read served as the low half, then the high half.
        fn whole(&self) -> u64 {
            let low = u64::from(self.low());
            u64::from(self.high()) << 32 | low
        }
    }

    #[test]
    fn a_split_read_across_the_low_wrap_is_torn() {
        let device = Splitting::new(0x1_FFFF_FFFE, 4);
        // Low half sampled at ...FFFE, high half at 0x2_0000_0002: 2^32 ahead.
        assert_eq!(device.whole(), 0x2_FFFF_FFFE);
    }

    #[test]
    fn a_torn_read_is_replaced_by_a_consistent_one() {
        let device = Splitting::new(0x1_FFFF_FFFE, 4);
        let last = 0x1_FFFF_FF00;
        let (ticks, split) = read(|| device.whole(), || device.low(), || device.high(), last);
        assert!(split);
        // The 64-bit read took two samples; the halves are high, low, high.
        assert_eq!(ticks, 0x2_0000_000A);
    }

    #[test]
    fn every_read_across_a_wrap_stays_close_to_the_true_count() {
        for offset in 0..64 {
            let start = 0x7_FFFF_FFE0 + offset;
            let device = Splitting::new(start, 3);
            let (ticks, _) = read(
                || device.whole(),
                || device.low(),
                || device.high(),
                start - 100,
            );
            assert!(
                ticks >= start && ticks < start + 32,
                "offset {offset}: {ticks:#x}"
            );
        }
    }

    #[test]
    fn reads_near_the_last_good_value_are_kept_either_side() {
        assert!(plausible(1_000, 900));
        assert!(plausible(900, 1_000));
        assert!(plausible(WINDOW - 1, 0));
        assert!(!plausible(WINDOW, 0));
        // A read torn across the wrap from 0x1_FFFF_FFF0 is 2^32 ahead.
        assert!(!plausible(0x2_FFFF_FFF5, 0x1_FFFF_FFF0));
        // Across a wrap of the low half, a consistent read is kept.
        assert!(plausible(0x1_0000_0010, 0x0_FFFF_FFF0));
    }

    #[test]
    fn halves_are_read_again_while_the_high_half_changes() {
        let device = Splitting::new(0xFFFF_FFFF, 1);
        assert_eq!(
            read_halves(|| device.low(), || device.high()),
            0x1_0000_0003
        );
    }

    #[test]
    fn a_32_bit_counter_is_carried_across_its_wraps() {
        assert_eq!(extend32(0x10, 0xFFFF_FFF0), 0x1_0000_0010);
        assert_eq!(extend32(0x5, 0x7_FFFF_FFFE), 0x8_0000_0005);
        // Another CPU's count may be a little ahead: the read stays behind it.
        assert_eq!(extend32(0xFFFF_FFF0, 0x1_0000_0010), 0xFFFF_FFF0);
        // Read every 2^30 ticks across 16 wraps, it follows the true count.
        let mut last = 0x7;
        for n in 1..64u64 {
            last = extend32((n << 30 | 0x7) as u32, last);
            assert_eq!(last, n << 30 | 0x7, "read {n}");
        }
    }

    /// Why a 32-bit counter needs [`extend32`]: its upper half reads 0, so after
    /// a wrap the 64-bit path drops 2^32 ticks the clock already counted.
    #[test]
    fn the_64_bit_path_drops_a_wrap_of_a_32_bit_counter() {
        let (low, last) = (0x10, 0xFFFF_FFF0);
        let got = read(|| u64::from(low), || low, || 0, last);
        assert_eq!(got, (0x10, true));
        assert_eq!(extend32(low, last), 0x1_0000_0010);
    }

    #[test]
    fn a_cpu_shares_the_boot_zero_only_when_its_tsc_agrees() {
        let hpet = 5_000_000_000;
        // Synced TSCs: two zero pairs' errors, either side (-22 ns to -16 us
        // in 49 of #1663's 51 own-zero KVM boots).
        assert!(shares_zero(hpet + 4_000, hpet));
        assert!(shares_zero(hpet, hpet));
        assert!(shares_zero(hpet - 1, hpet));
        assert!(shares_zero(hpet - 16_000, hpet));
        assert!(shares_zero(hpet + SHARED_ZERO_NS, hpet));
        assert!(shares_zero(hpet - SHARED_ZERO_NS, hpet));
        // A TSC that started later or earlier than the boot CPU's.
        assert!(!shares_zero(hpet + SHARED_ZERO_NS + 1, hpet));
        assert!(!shares_zero(hpet - SHARED_ZERO_NS - 1, hpet));
        assert!(!shares_zero(hpet - 3_000_000, hpet));
        assert!(!shares_zero(u64::MAX, hpet));
    }

    #[test]
    fn the_zero_pair_is_the_quickest_read_at_its_middle() {
        let samples = [
            (1_000, 70, 9_000),
            (20_000, 75, 21_000),
            (30_000, 80, 90_000),
        ];
        assert_eq!(best_pair(samples), Some((20_500, 75)));
        assert_eq!(best_pair([]), None);
        // The first of equally quick reads.
        assert_eq!(best_pair([(0, 1, 10), (100, 2, 110)]), Some((5, 1)));
    }

    #[test]
    fn the_capabilities_word_gives_the_counter_size() {
        // QEMU's HPET (64-bit), then a 14.318 MHz one with bit 13 clear.
        assert!(counter_is_64bit(0x0098_9680_8086_A201));
        assert!(!counter_is_64bit(0x0429_B17F_1022_8210));
    }

    #[test]
    fn a_rate_is_settled_only_when_steady_and_near_the_reported_one() {
        let ghz = 2_700_000_000;
        assert!(settled(ghz, ghz + 1_000, None), "steady, nothing reported");
        // A 0.6% step between windows is refused; 0.4% is taken.
        assert!(!settled(ghz + ghz / 1000 * 6, ghz, None));
        assert!(settled(ghz + ghz / 1000 * 4, ghz, None));
        assert!(!settled(0, 0, None), "a rate of 0");
        assert!(
            settled(ghz, ghz, Some(ghz + ghz / 200)),
            "0.5% from the report"
        );
        assert!(
            !settled(ghz, ghz, Some(ghz + ghz / 50)),
            "2% from the report"
        );
    }

    #[test]
    fn a_recheck_measures_the_tsc_clock_against_the_hpet() {
        let s = 1_000_000_000;
        assert_eq!(recheck_ppm((0, 10 * s), (0, 10 * s)), 0);
        // 10 ms fast or slow over 10 s.
        assert_eq!(
            recheck_ppm((5, 10 * s + 10_000_005), (7, 10 * s + 7)),
            1_000
        );
        assert_eq!(recheck_ppm((0, 10 * s - 10_000_000), (0, 10 * s)), 1_000);
        // A clock that went back, or an HPET that did not move, cannot panic.
        assert_eq!(recheck_ppm((9, 1), (0, s)), 1_000_000);
        assert!(recheck_ppm((0, s), (5, 5)) > 1_000);
    }
}
