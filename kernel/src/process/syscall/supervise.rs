//! The supervisor's calls: spawn, wait, kill, its saved state, the plan and the
//! list of services; PROC_STOP for the supervisor, and PROC_STOP_ONCE for a Job
//! handle with STOP and the user's `ONCE`.

use super::need;
use crate::process::supervision;
use kabi::{Error, call, cap};

/// The calls this area serves.
const CALLS: &[u64] = &[
    call::SPAWN,
    call::WAIT_CHILD,
    call::KILL,
    call::STATE_SAVE,
    call::STATE_LOAD,
    call::PLAN,
    call::LIST,
    call::PROC_STOP,
    call::PROC_STOP_ONCE,
];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| run(slot, number, a))
}

fn run(slot: usize, number: u64, a: &[u64; 6]) -> Result<u64, Error> {
    match number {
        call::SPAWN => {
            need(slot, cap::SUPERVISE)?;
            supervision::spawn_service(slot, a[0])
        }
        call::WAIT_CHILD => {
            need(slot, cap::SUPERVISE)?;
            supervision::wait_child(slot, a[0], a[1])
        }
        call::KILL => {
            need(slot, cap::SUPERVISE)?;
            supervision::kill_service(a[0])
        }
        call::STATE_SAVE => {
            need(slot, cap::SUPERVISE)?;
            supervision::state_save(slot, a[0], a[1])
        }
        call::STATE_LOAD => {
            need(slot, cap::SUPERVISE)?;
            supervision::state_load(slot, a[0], a[1])
        }
        call::PLAN => {
            need(slot, cap::SUPERVISE)?;
            supervision::plan(slot, a[0], a[1])
        }
        call::LIST => {
            need(slot, cap::SUPERVISE)?;
            supervision::list(slot, a[0], a[1])
        }
        call::PROC_STOP => {
            need(slot, cap::SUPERVISE)?;
            supervision::proc_stop(slot, a[0], a[1])
        }
        call::PROC_STOP_ONCE => {
            need(slot, cap::STOP)?;
            supervision::proc_stop_once(slot, a[0], a[1], a[2], a[3])
        }
        _ => Err(Error::NotFound),
    }
}
