//! Jobs that start one program (ADR 0001 D6): JOB_SPAWN, JOB_WAIT and the pool's
//! JOB_POOL and JOB_CLAIM, each on a `Job` handle the caller names; and the
//! pooled child's JOB_TAKE and JOB_DONE.

use crate::process::job;
use kabi::{Error, call};

/// The calls this area serves.
const CALLS: &[u64] = &[
    call::JOB_SPAWN,
    call::JOB_WAIT,
    call::JOB_POOL,
    call::JOB_CLAIM,
    call::JOB_TAKE,
    call::JOB_DONE,
    call::JOB_KILL,
];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| match number {
        call::JOB_SPAWN => job::spawn(slot, a[0], a[1]),
        call::JOB_WAIT => job::wait(slot, a[0], a[1], a[2]),
        call::JOB_POOL => job::pool(slot, a[0], a[1]),
        call::JOB_CLAIM => job::claim(slot, a[0], a[1]),
        call::JOB_TAKE => job::take(slot, a[0], a[1]),
        call::JOB_DONE => job::done(slot, a[0]),
        _ => job::kill_child(slot, a[0], a[1]),
    })
}
