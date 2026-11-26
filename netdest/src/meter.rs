//! A destination's bytes a day (P5 task 14, ADR 0013 section 6): an agent's
//! `NetDest` moves at most its manifest's `bytes/day` (10 MiB unless the manifest
//! asks for more, never past 512 MiB: `kaiagent::NetBudget`), counted by the kernel
//! on the handle. This is the counting rule, with no kernel in it; the kernel keeps
//! one [`DayMeter`] per agent handle and asks it on each send and receive.
//!
//! - **Both ways count.** What is sent and what is received are one day's bytes.
//! - **A send that would pass the budget is refused whole**, before it leaves, and
//!   charges nothing ([`DayMeter::try_send`]).
//! - **What is received is counted when it arrives** ([`DayMeter::received`]): it
//!   cannot be refused once it is here, so a receive may pass the budget by its own
//!   bytes, and then nothing more moves that day: the kernel refuses the next
//!   receive, as it does a send and a connect, once [`DayMeter::has_room`] is false.
//! - **The day turns forward only.** A later day starts the count again; a clock
//!   that goes back is not a new day, so it never buys more bytes.
//! - **No clock is no day** ([`NO_CLOCK`]): what moves before the wall clock is
//!   read counts toward the first day it shows, never a day of its own.

/// Bytes in a MiB, as `bytes/day=10m` counts them.
pub const MIB: u64 = 1024 * 1024;

/// The day a kernel with no wall clock yet counts on.
pub const NO_CLOCK: u32 = 0;

/// Why a send was refused: the day's bytes so far and the budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverBudget {
    pub used: u64,
    pub budget: u64,
}

/// One handle's bytes on one day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DayMeter {
    budget: u64,
    used: u64,
    day: u32,
}

impl DayMeter {
    /// A meter of `budget` bytes a day, starting on `day`.
    pub const fn new(budget: u64, day: u32) -> Self {
        Self {
            budget,
            used: 0,
            day,
        }
    }

    /// A meter of `mib` MiB a day (a manifest's `bytes/day=<mib>m`), from `day`.
    pub const fn from_mib(mib: u32, day: u32) -> Self {
        Self::new(mib as u64 * MIB, day)
    }

    /// A later `day` starts the count again; an earlier one is the same day.
    /// The first day after [`NO_CLOCK`] keeps what was counted with no clock.
    fn roll(&mut self, day: u32) {
        if day > self.day {
            if self.day != NO_CLOCK {
                self.used = 0;
            }
            self.day = day;
        }
    }

    /// Whether `bytes` may be sent on `day`: charged if so, refused whole if they
    /// would pass the budget.
    pub fn try_send(&mut self, bytes: u64, day: u32) -> Result<(), OverBudget> {
        self.roll(day);
        match self.used.checked_add(bytes) {
            Some(after) if after <= self.budget => {
                self.used = after;
                Ok(())
            }
            _ => Err(OverBudget {
                used: self.used,
                budget: self.budget,
            }),
        }
    }

    /// Gives back `bytes` of a send [`DayMeter::try_send`] charged on `day` that
    /// did not leave (a stream that took only part of it). Never below 0, and
    /// nothing once the meter has moved past `day`: the charge was the day
    /// before's, and the new day's count never pays it back (a clock gone back
    /// gets none either: it can only charge more, never less).
    pub fn refund(&mut self, bytes: u64, day: u32) {
        if day >= self.day {
            self.used = self.used.saturating_sub(bytes);
        }
    }

    /// Counts `bytes` received on `day`: whether the day's budget still has room.
    pub fn received(&mut self, bytes: u64, day: u32) -> bool {
        self.roll(day);
        self.used = self.used.saturating_add(bytes);
        self.used < self.budget
    }

    /// Whether anything more may move on `day`.
    pub fn has_room(&mut self, day: u32) -> bool {
        self.roll(day);
        self.used < self.budget
    }

    /// The day's bytes so far.
    pub fn used(&self) -> u64 {
        self.used
    }

    pub fn budget(&self) -> u64 {
        self.budget
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_within_the_budget_are_charged_and_one_past_it_is_refused_whole() {
        let mut meter = DayMeter::from_mib(10, 1);
        assert_eq!(meter.budget(), 10 * MIB);
        assert_eq!(meter.try_send(6 * MIB, 1), Ok(()));
        assert_eq!(
            meter.try_send(5 * MIB, 1),
            Err(OverBudget {
                used: 6 * MIB,
                budget: 10 * MIB
            })
        );
        assert_eq!(meter.used(), 6 * MIB, "a refused send charges nothing");
        assert_eq!(
            meter.try_send(4 * MIB, 1),
            Ok(()),
            "exactly the budget fits"
        );
        assert!(!meter.has_room(1));
        assert!(meter.try_send(1, 1).is_err());
    }

    #[test]
    fn what_is_received_counts_and_stops_the_day_once_past() {
        let mut meter = DayMeter::from_mib(1, 7);
        assert!(meter.received(MIB / 2, 7));
        // A receive cannot be refused once here: it passes, and the day is done.
        assert!(!meter.received(MIB, 7));
        assert_eq!(meter.used(), MIB / 2 + MIB);
        assert!(meter.try_send(1, 7).is_err());
        assert!(!meter.has_room(7));
    }

    #[test]
    fn a_later_day_starts_again_and_an_earlier_one_buys_nothing() {
        let mut meter = DayMeter::from_mib(1, 10);
        assert!(meter.try_send(MIB, 10).is_ok());
        assert!(
            meter.try_send(1, 9).is_err(),
            "a clock gone back is the same day"
        );
        assert!(meter.try_send(MIB, 11).is_ok(), "the next day starts again");
        assert_eq!(meter.used(), MIB);
    }

    #[test]
    fn no_budget_moves_nothing_and_a_huge_send_does_not_overflow() {
        // A send taken in part gives the rest back.
        let mut part = DayMeter::from_mib(1, 1);
        assert!(part.try_send(1000, 1).is_ok());
        part.refund(400, 1);
        assert_eq!(part.used(), 600);
        part.refund(u64::MAX, 1);
        assert_eq!(part.used(), 0);
        let mut none = DayMeter::from_mib(0, 1);
        assert!(none.try_send(1, 1).is_err());
        assert!(!none.has_room(1));
        let mut max = DayMeter::from_mib(512, 1);
        assert_eq!(max.budget(), 512 * MIB);
        assert!(max.try_send(u64::MAX, 1).is_err());
        assert_eq!(max.used(), 0);
        assert!(!max.received(u64::MAX, 1));
        assert_eq!(max.used(), u64::MAX);
    }

    #[test]
    fn a_refund_after_the_day_turned_gives_the_new_day_nothing() {
        let mut meter = DayMeter::from_mib(1, 5);
        assert!(meter.try_send(1000, 5).is_ok());
        // Another send rolls the meter to day 6 before the first one's refund.
        assert!(meter.try_send(300, 6).is_ok());
        meter.refund(1000, 5);
        assert_eq!(meter.used(), 300, "day 5's refund is not day 6's");
        meter.refund(100, 6);
        assert_eq!(meter.used(), 200);
    }

    #[test]
    fn bytes_moved_before_the_clock_count_toward_its_first_day() {
        let mut meter = DayMeter::from_mib(1, NO_CLOCK);
        assert!(meter.try_send(MIB, NO_CLOCK).is_ok());
        // The clock is read: the first real day is not a fresh budget.
        assert!(meter.try_send(1, 20_000).is_err());
        assert_eq!(meter.used(), MIB);
        // The day after it is.
        assert!(meter.try_send(1, 20_001).is_ok());
        assert_eq!(meter.used(), 1);
    }
}
