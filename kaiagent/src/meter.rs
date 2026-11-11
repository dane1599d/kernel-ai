//! A day's bytes counted against a budget: the counter for [`crate::NetBudget`]
//! (ADR 0013 section 6), pure, so the network service and the host tests count the
//! same way.
//!
//! It has no clock. The caller says which day it is (`day`, the host's calendar
//! day, which turns at midnight as `agentsched`'s does) with every call, and a day
//! later than the last one starts the count again at zero. A date set back does not
//! give the day back: a `day` earlier than the last one counts as the last one. Going
//! forward is trusted, though: every step forward pays a fresh budget, and a clock
//! set a day ahead starts one at once, with no reset when the calendar catches up
//! (the counter cannot tell a real midnight from a set clock; the host owns that).
//!
//! A charge that would pass the limit is refused whole and counts nothing, so a
//! transfer stops at the byte it cannot pay for, and a charge of zero is always paid,
//! even over a limit that was lowered below what was used. The limit can change at
//! any time (the person raises or lowers it): what was used today stays.

use core::fmt;

/// Why a charge was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Over {
    /// The day's limit, in bytes.
    pub limit: u64,
    /// Bytes used today before this charge.
    pub used: u64,
    /// Bytes the charge asked for.
    pub wanted: u64,
}

impl fmt::Display for Over {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Over budget: {} of {} bytes used today, {} more asked",
            self.used, self.limit, self.wanted
        )
    }
}

/// Bytes moved today against a limit. It is not `Copy`: a charge on a copy would be
/// lost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DayBytes {
    limit: u64,
    /// The last day charged, if any.
    day: Option<u32>,
    used: u64,
}

impl DayBytes {
    pub const fn new(limit: u64) -> Self {
        Self {
            limit,
            day: None,
            used: 0,
        }
    }

    pub const fn limit(&self) -> u64 {
        self.limit
    }

    /// Changes the limit; what was used today stays.
    pub fn set_limit(&mut self, limit: u64) {
        self.limit = limit;
    }

    /// Bytes used on `day`: none if it is later than the last day charged.
    pub fn used(&self, day: u32) -> u64 {
        match self.day {
            Some(last) if day <= last => self.used,
            _ => 0,
        }
    }

    /// Bytes still to spend on `day`.
    pub fn left(&self, day: u32) -> u64 {
        self.limit.saturating_sub(self.used(day))
    }

    /// Counts `bytes` against `day`, or refuses them whole.
    pub fn charge(&mut self, day: u32, bytes: u64) -> Result<(), Over> {
        // A day later than the last starts again; an earlier one is the last.
        if self.day.is_none_or(|last| day > last) {
            self.day = Some(day);
            self.used = 0;
        }
        // A charge that moves nothing is paid, whatever the limit now is.
        if bytes == 0 {
            return Ok(());
        }
        match self.used.checked_add(bytes) {
            Some(total) if total <= self.limit => {
                self.used = total;
                Ok(())
            }
            _ => Err(Over {
                limit: self.limit,
                used: self.used,
                wanted: bytes,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn charges_add_up_to_the_limit_and_the_next_byte_is_refused_whole() {
        let mut meter = DayBytes::new(100);
        assert_eq!(meter.charge(5, 60), Ok(()));
        assert_eq!(meter.charge(5, 40), Ok(()));
        assert_eq!((meter.used(5), meter.left(5)), (100, 0));
        let over = meter.charge(5, 1).unwrap_err();
        assert_eq!(
            over,
            Over {
                limit: 100,
                used: 100,
                wanted: 1
            }
        );
        // A refused charge counts nothing, and a smaller one that fits is paid.
        let mut meter = DayBytes::new(100);
        meter.charge(5, 90).unwrap();
        assert!(meter.charge(5, 11).is_err());
        assert_eq!(meter.used(5), 90);
        assert_eq!(meter.charge(5, 10), Ok(()));
    }

    #[test]
    fn a_later_day_starts_again_and_an_earlier_one_is_the_last() {
        let mut meter = DayBytes::new(100);
        meter.charge(10, 100).unwrap();
        assert_eq!(meter.charge(10, 1).map_err(|o| o.used), Err(100));
        // Midnight: a new day.
        assert_eq!((meter.used(11), meter.left(11)), (0, 100));
        assert_eq!(meter.charge(11, 100), Ok(()));
        // The clock goes back: yesterday does not give the budget back.
        assert!(meter.charge(10, 1).is_err());
        assert_eq!(meter.used(10), 100);
        assert_eq!(meter.used(9), 100);
    }

    #[test]
    fn zero_is_always_paid_and_a_huge_charge_cannot_wrap_the_count() {
        let mut meter = DayBytes::new(0);
        assert_eq!(meter.charge(1, 0), Ok(()));
        assert!(meter.charge(1, 1).is_err());
        let mut meter = DayBytes::new(u64::MAX);
        meter.charge(1, u64::MAX - 1).unwrap();
        // used + wanted overflows u64: refused, not wrapped to a small number.
        assert!(meter.charge(1, u64::MAX).is_err());
        assert_eq!(meter.charge(1, 1), Ok(()));
        assert!(meter.charge(1, 1).is_err());
    }

    #[test]
    fn the_limit_changes_and_what_was_used_stays() {
        let mut meter = DayBytes::new(100);
        meter.charge(1, 80).unwrap();
        meter.set_limit(50);
        assert_eq!((meter.limit(), meter.left(1)), (50, 0));
        assert!(meter.charge(1, 1).is_err());
        meter.set_limit(200);
        assert_eq!(meter.left(1), 120);
        assert_eq!(meter.charge(1, 120), Ok(()));
    }

    #[test]
    fn a_refusal_says_the_numbers() {
        let mut meter = DayBytes::new(10);
        meter.charge(1, 8).unwrap();
        let said = meter.charge(1, 5).unwrap_err().to_string();
        assert_eq!(said, "Over budget: 8 of 10 bytes used today, 5 more asked");
    }

    #[test]
    fn a_zero_charge_is_paid_even_over_a_limit_lowered_below_what_was_used() {
        let mut meter = DayBytes::new(100);
        meter.charge(1, 90).unwrap();
        meter.set_limit(50);
        assert_eq!(meter.charge(1, 0), Ok(()));
        assert_eq!(
            meter.charge(1, 1).map_err(|o| (o.limit, o.used, o.wanted)),
            Err((50, 90, 1))
        );
        assert_eq!((meter.used(1), meter.left(1)), (90, 0));
        // And at the limit exactly, with a limit above zero.
        let mut meter = DayBytes::new(10);
        meter.charge(1, 10).unwrap();
        assert_eq!(meter.charge(1, 0), Ok(()));
        assert!(meter.charge(1, 1).is_err());
        // A zero charge on a later day still turns the day.
        assert_eq!(meter.charge(2, 0), Ok(()));
        assert_eq!(meter.used(2), 0);
    }

    #[test]
    fn a_paid_charge_on_an_earlier_day_adds_to_the_last_days_count() {
        let mut meter = DayBytes::new(100);
        meter.charge(5, 60).unwrap();
        // Back a day: it counts as day 5.
        meter.charge(4, 30).unwrap();
        assert_eq!(meter.used(5), 90);
        assert!(meter.charge(4, 11).is_err());
        // Forward to day 5 again: the same day, not a new one.
        meter.charge(5, 10).unwrap();
        assert_eq!(meter.used(5), 100);
        assert!(meter.charge(5, 1).is_err());
    }

    #[test]
    fn each_step_forward_pays_a_fresh_budget_and_a_jump_ahead_starts_one() {
        let mut meter = DayBytes::new(10);
        for day in 1..=3 {
            assert_eq!(meter.charge(day, 10), Ok(()), "{day}");
        }
        // A clock set ahead starts a budget at once, and the real day, behind it, is
        // the last one: no reset when the calendar catches up.
        assert_eq!(meter.charge(30, 10), Ok(()));
        assert!(meter.charge(4, 1).is_err());
        assert_eq!(meter.used(4), 10);
    }
}
