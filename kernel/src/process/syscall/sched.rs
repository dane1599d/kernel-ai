//! The scheduler: the policy, its numbers and kernel-owned trials.

use super::need;
use kabi::{Error, call, cap};

/// The calls this area serves.
const CALLS: &[u64] = &[call::POLICY, call::SCHED_STATS, call::SCHED_TRIAL];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| run(slot, number, a))
}

fn run(slot: usize, number: u64, a: &[u64; 6]) -> Result<u64, Error> {
    match number {
        call::POLICY => {
            need(slot, cap::POLICY)?;
            crate::sched_control::calls::policy(a[0], a[1])
        }
        call::SCHED_STATS => {
            need(slot, cap::POLICY)?;
            crate::sched_control::calls::stats(slot, a[0], a[1])
        }
        call::SCHED_TRIAL => {
            need(slot, cap::POLICY)?;
            crate::sched_control::calls::trial(slot, a[0])
        }
        _ => Err(Error::NotFound),
    }
}
