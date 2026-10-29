//! Burst accounting for process threads (L6), on `sched::burst`'s host-tested
//! rules: each thread's CPU time since it last blocked, charged on every switch and
//! tick, and a Runaway kill once it passes its process's `max_burst_ms`.
//!
//! - Per thread: a worker computing does not use up its main thread's burst, and a
//!   thread blocking restarts only its own. A thread's count starts at 0 when it
//!   takes its tid, before its slot is published, so an earlier owner's runaway
//!   count can never kill it.
//! - The compute allowance: while a process serves a call that is still open (it
//!   took it, its caller still waits, and its deadline has not passed), the time its
//!   threads run is not charged. A service may compute as long as someone waits for
//!   its answer; once the call is answered, dropped or past its deadline, the limit
//!   applies again, so a hung service is still caught.
//! - Runaway is decided on the timer tick, for the thread running. The switch that
//!   takes a thread off its CPU only adds its time: a thread that is going to sleep
//!   has already checked for a kill, and a kill set then would wake nobody.

use super::{MAX_PROCESSES, request_kill, slot_of};
use crate::{hpet, percpu, threads};
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use kabi::ExitReason;
use sched::{BurstTable, Tid};

static BURSTS: BurstTable = BurstTable::new();
/// Each process's burst limit (0 = none).
static LIMIT_NS: [AtomicU64; MAX_PROCESSES] = [const { AtomicU64::new(0) }; MAX_PROCESSES];
/// The call each process serves: its IPC call slot plus one (0 = none), the call's
/// id, and its deadline. Written by the serving thread, read from any CPU's tick.
static SERVING_CALL: [AtomicUsize; MAX_PROCESSES] = [const { AtomicUsize::new(0) }; MAX_PROCESSES];
static SERVING_ID: [AtomicU64; MAX_PROCESSES] = [const { AtomicU64::new(0) }; MAX_PROCESSES];
static SERVING_UNTIL: [AtomicU64; MAX_PROCESSES] = [const { AtomicU64::new(0) }; MAX_PROCESSES];
/// CPU time each process's threads have used since it started (METRICS), every
/// charge counted, exempt or not.
static CPU_NS: [AtomicU64; MAX_PROCESSES] = [const { AtomicU64::new(0) }; MAX_PROCESSES];

/// A new process in `slot`: its limit, and no call served.
pub fn start_process(slot: usize, limit_ms: u64) {
    LIMIT_NS[slot].store(limit_ms.saturating_mul(1_000_000), Ordering::Relaxed);
    CPU_NS[slot].store(0, Ordering::Relaxed);
    SERVING_CALL[slot].store(0, Ordering::Release);
}

/// A thread took `tid`: its burst starts at 0. Called before its slot is published.
pub fn start_thread(tid: Tid) {
    BURSTS.start(tid);
}

/// The thread `tid` just came back from blocking or sleeping: its burst starts over.
pub fn blocked(tid: Tid) {
    BURSTS.blocked(tid);
}

/// The process in `slot` took IPC call `call` (slot, id and deadline), or, with
/// None, answered it.
pub fn serving(slot: usize, call: Option<(usize, u64, u64)>) {
    match call {
        Some((index, id, deadline)) => {
            SERVING_CALL[slot].store(0, Ordering::Release);
            SERVING_ID[slot].store(id, Ordering::Relaxed);
            SERVING_UNTIL[slot].store(deadline, Ordering::Relaxed);
            SERVING_CALL[slot].store(index + 1, Ordering::Release);
        }
        None => SERVING_CALL[slot].store(0, Ordering::Release),
    }
}

/// Until when the process in `slot` is exempt: the deadline of the call it serves
/// while that call is still open, else 0. A torn read across a change of call only
/// charges or exempts one tick.
fn serving_until(slot: usize) -> u64 {
    let call = SERVING_CALL[slot].load(Ordering::Acquire);
    if call == 0 {
        return 0;
    }
    let id = SERVING_ID[slot].load(Ordering::Relaxed);
    let until = SERVING_UNTIL[slot].load(Ordering::Relaxed);
    if crate::ipc::call_open(call - 1, id) {
        until
    } else {
        0
    }
}

/// Charges CPU time since the last charge on this CPU to `tid`, and on the tick
/// (`decide`) kills its process once the burst is past the limit. Interrupts are off.
fn charge(tid: Tid, decide: bool) {
    let now = hpet::nanos();
    let since = percpu::this().charged_at_ns.swap(now, Ordering::Relaxed);
    let Some(slot) = slot_of(tid) else {
        return;
    };
    CPU_NS[slot].fetch_add(now.saturating_sub(since), Ordering::Relaxed);
    let limit = LIMIT_NS[slot].load(Ordering::Relaxed);
    let (_, runaway) = BURSTS.charge(tid, since, now, serving_until(slot), limit);
    if decide && runaway {
        request_kill(slot, ExitReason::Runaway, limit / 1_000_000);
    }
}

/// CPU time the process in `slot` has used since it started, in nanoseconds.
pub fn cpu_ns(slot: usize) -> u64 {
    CPU_NS[slot].load(Ordering::Relaxed)
}

/// `tid` leaves its CPU (called by every context switch): only adds its time.
pub fn switched_out(tid: Tid) {
    charge(tid, false);
}

/// Timer-tick hook: charges the running thread and decides Runaway.
pub fn on_tick() {
    charge(threads::current_tid(), true);
}
