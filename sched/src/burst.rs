//! Burst accounting (L6): how long each thread of a process has run since it last
//! blocked, and when that makes it a runaway. Lock-free, so the kernel can charge
//! from the timer interrupt.
//!
//! - Per thread: a worker computing does not use up its main thread's burst, and a
//!   thread that blocks restarts only its own.
//! - A thread's count starts at 0 when it takes its id ([`BurstTable::start`]),
//!   whatever the id's last owner left there.
//! - The compute allowance: time a process spends while it serves a call someone
//!   still waits for counts only past that call's deadline ([`chargeable`]), so a
//!   service may compute as long as its caller is willing to wait, and a hung one is
//!   still caught once the deadline has passed.

use crate::{MAX_THREADS, Tid};
use core::sync::atomic::{AtomicU64, Ordering};

/// The part of a stretch on the CPU from `since` to `now` that counts toward a
/// thread's burst, when its process serves an open call until `serving_until` (0
/// when it serves none): the part after that deadline, all of it without one.
pub fn chargeable(since: u64, now: u64, serving_until: u64) -> u64 {
    now.saturating_sub(since.max(serving_until))
}

/// Whether a burst of `burst_ns` is past a limit of `limit_ns`; 0 means no limit.
pub fn runaway(burst_ns: u64, limit_ns: u64) -> bool {
    limit_ns != 0 && burst_ns > limit_ns
}

/// Every thread's burst, in nanoseconds.
pub struct BurstTable {
    counts: [AtomicU64; MAX_THREADS],
}

impl BurstTable {
    pub const fn new() -> Self {
        Self {
            counts: [const { AtomicU64::new(0) }; MAX_THREADS],
        }
    }

    /// A thread took `tid`: its burst starts at 0. Call before anything may charge
    /// the thread.
    pub fn start(&self, tid: Tid) {
        self.counts[tid].store(0, Ordering::Relaxed);
    }

    /// `tid` came back from blocking or sleeping: its burst starts over.
    pub fn blocked(&self, tid: Tid) {
        self.counts[tid].store(0, Ordering::Relaxed);
    }

    /// Adds what counts of `since..now` to `tid`'s burst; returns the burst and
    /// whether it is now past `limit_ns`. Only the CPU running `tid` charges it.
    pub fn charge(
        &self,
        tid: Tid,
        since: u64,
        now: u64,
        serving_until: u64,
        limit_ns: u64,
    ) -> (u64, bool) {
        let used = chargeable(since, now, serving_until);
        let burst = self.counts[tid].fetch_add(used, Ordering::Relaxed) + used;
        (burst, runaway(burst, limit_ns))
    }

    pub fn get(&self, tid: Tid) -> u64 {
        self.counts[tid].load(Ordering::Relaxed)
    }
}

impl Default for BurstTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn time_serving_an_open_call_counts_only_past_its_deadline() {
        assert_eq!(chargeable(10 * MS, 15 * MS, 0), 5 * MS);
        assert_eq!(chargeable(10 * MS, 15 * MS, 20 * MS), 0);
        assert_eq!(chargeable(10 * MS, 15 * MS, 12 * MS), 3 * MS);
        assert_eq!(chargeable(10 * MS, 15 * MS, 15 * MS), 0);
        assert_eq!(chargeable(15 * MS, 10 * MS, 0), 0);
    }

    #[test]
    fn a_limit_of_zero_never_kills() {
        assert!(!runaway(u64::MAX, 0));
        assert!(!runaway(300 * MS, 300 * MS));
        assert!(runaway(300 * MS + 1, 300 * MS));
    }

    #[test]
    fn threads_are_charged_apart_and_each_restarts_alone() {
        let table = BurstTable::new();
        let limit = 300 * MS;
        // Two workers and a main thread each run 200 ms: 600 ms in all, none a runaway.
        for tid in [3, 4, 5] {
            table.start(tid);
            assert_eq!(table.charge(tid, 0, 200 * MS, 0, limit), (200 * MS, false));
        }
        table.blocked(4);
        assert_eq!(table.get(4), 0);
        assert_eq!(table.get(3), 200 * MS);
        // Thread 3 goes on past the limit; 5 serves a call open until 1 s, and is
        // charged only after it.
        assert!(table.charge(3, 200 * MS, 301 * MS, 0, limit).1);
        assert_eq!(
            table.charge(5, 200 * MS, 900 * MS, 1000 * MS, limit),
            (200 * MS, false)
        );
        assert!(table.charge(5, 900 * MS, 1101 * MS, 1000 * MS, limit).1);
    }

    #[test]
    fn a_reused_id_starts_at_zero() {
        let table = BurstTable::new();
        let limit = 300 * MS;
        assert!(table.charge(7, 0, 900 * MS, 0, limit).1);
        // The runaway's id goes to a new thread, which must not die on its first tick.
        table.start(7);
        assert_eq!(table.charge(7, 900 * MS, 901 * MS, 0, limit), (MS, false));
    }
}
