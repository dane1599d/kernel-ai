//! The trial's guardrail, run by every CPU's timer tick with its core locked:
//! atomics only, and the Core's allocation-free revert. A breach reverts that
//! CPU at the first tick past it and asks every other CPU to revert at its next.

use crate::percpu::MAX_CPUS;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use kabi::sched::reason;
use sched::Core;
use sched::judge::GUARDRAIL_WAIT_NS;

/// How a guardrail or an abort ended the trial.
#[derive(Clone, Copy, Debug, Default)]
pub struct Trip {
    pub reason: u64,
    /// The thread that waited (guardrail), or the process (kill, exit).
    pub tid: u64,
    pub pid: u64,
    /// The wait when the CPU reverted, and at the tick before.
    pub wait_ns: u64,
    pub previous_wait_ns: u64,
}

/// Candidate windows are running: the tick guards them.
pub(super) static ARMED: AtomicBool = AtomicBool::new(false);
/// Every CPU reverts at its next tick, and the driver stops.
pub(super) static ABORT: AtomicBool = AtomicBool::new(false);
pub(super) static TRIP_REASON: AtomicU64 = AtomicU64::new(0);
static TRIP_TID: AtomicU64 = AtomicU64::new(0);
pub(super) static TRIP_PID: AtomicU64 = AtomicU64::new(0);
static TRIP_WAIT: AtomicU64 = AtomicU64::new(0);
static TRIP_PREVIOUS: AtomicU64 = AtomicU64::new(0);
/// Each CPU's longest wait at its last guarded tick.
pub(super) static LAST_WAIT: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Called by every CPU's timer tick with its core locked. Returns true if it
/// reverted the core, so the caller picks again. Never allocates.
pub fn guard(cpu: usize, core: &mut Core, now: u64) -> bool {
    if !ARMED.load(Ordering::Acquire) || !core.in_trial() {
        return false;
    }
    let (wait, tid) = core.longest_wait(now);
    let last = &LAST_WAIT[cpu.min(MAX_CPUS - 1)];
    let code = if ABORT.load(Ordering::Acquire) {
        reason::NONE
    } else if core.on_fallback() {
        reason::EJECTED
    } else if wait > GUARDRAIL_WAIT_NS {
        reason::GUARDRAIL
    } else {
        last.store(wait, Ordering::Relaxed);
        return false;
    };
    if !core.revert(now) {
        return false;
    }
    if code != reason::NONE && claim(code) {
        TRIP_TID.store(tid as u64, Ordering::Relaxed);
        TRIP_WAIT.store(wait, Ordering::Relaxed);
        TRIP_PREVIOUS.store(last.load(Ordering::Relaxed), Ordering::Relaxed);
    }
    ABORT.store(true, Ordering::Release);
    true
}

/// Records the first reason only.
pub(super) fn claim(code: u64) -> bool {
    TRIP_REASON
        .compare_exchange(0, code, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

/// The latest trip.
pub fn last_trip() -> Trip {
    Trip {
        reason: TRIP_REASON.load(Ordering::Acquire),
        tid: TRIP_TID.load(Ordering::Relaxed),
        pid: TRIP_PID.load(Ordering::Relaxed),
        wait_ns: TRIP_WAIT.load(Ordering::Relaxed),
        previous_wait_ns: TRIP_PREVIOUS.load(Ordering::Relaxed),
    }
}
