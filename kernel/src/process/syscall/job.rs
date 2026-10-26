//! Jobs that start one program (ADR 0001 D6): JOB_SPAWN and JOB_WAIT, each on a
//! `Job` handle the caller names.

use crate::process::job;
use kabi::{Error, call};

/// The calls this area serves.
const CALLS: &[u64] = &[call::JOB_SPAWN, call::JOB_WAIT];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| match number {
        call::JOB_SPAWN => job::spawn(slot, a[0], a[1]),
        _ => job::wait(slot, a[0], a[1], a[2]),
    })
}
