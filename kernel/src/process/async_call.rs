//! One call in flight per process, not waited for (CALL_SUBMIT, CALL_POLL):
//! the shell sends `act` to `llm` and keeps serving app reads until the reply
//! is in, on one thread (no THREADS, no worker IPC).
//!
//! - The call is an ordinary IPC call ([`ipc::submit_with`]) whose caller does
//!   not wait. Its reply wakes the caller's thread, as any reply does; a thread
//!   blocked in `svc_recv` or `RECV_H` then returns Woken ([`wakes`]), as for an
//!   event box, when no request is queued.
//! - It ends when CALL_POLL collects it: its reply, or why there is none
//!   (timed out, the server died, or the person's Stop). Its deadline passing,
//!   or a Stop, also wakes the caller, so a poll answers then.
//! - A process that exits drops its call: the late reply goes nowhere.

use super::MAX_PROCESSES;
use crate::hpet;
use crate::ipc::{self, CallError, Pending};
use alloc::vec::Vec;
use captable::Entry;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

/// Each process's call in flight, each behind a lock of its own: only that
/// process's threads take it.
static CALLS: [Mutex<Option<Pending>>; MAX_PROCESSES] = [const { Mutex::new(None) }; MAX_PROCESSES];
/// Its call slot plus one (0 = none) and its deadline, read without the lock
/// by a wait's wake-up test.
static SLOT: [AtomicUsize; MAX_PROCESSES] = [const { AtomicUsize::new(0) }; MAX_PROCESSES];
static DEADLINE_NS: [AtomicU64; MAX_PROCESSES] = [const { AtomicU64::new(0) }; MAX_PROCESSES];
/// Set when a Stop was taken by another of the process's calls, or by its
/// `interrupted()`, while this one was in flight: it ends Interrupted too (lane T
/// on #888).
static STOPPED: [AtomicBool; MAX_PROCESSES] = [const { AtomicBool::new(false) }; MAX_PROCESSES];

/// What a poll found.
pub enum Polled {
    /// The call is still in flight.
    InFlight,
    /// The reply, the pid that answered and the handles it moved, or why none.
    Ended(Result<(Vec<u8>, u64, Vec<Entry>), CallError>),
}

/// Sends `request` to `name` for the process in `slot` (pid `pid`), not waited
/// for. Busy while it has one in flight.
pub fn submit(
    slot: usize,
    name: &str,
    request: &[u8],
    pid: u64,
    deadline_ns: u64,
    extra: ipc::Extra,
) -> Result<(), CallError> {
    let mut call = CALLS[slot].lock();
    if call.is_some() {
        return Err(CallError::Busy);
    }
    let pending = ipc::submit_with(name, request, pid, deadline_ns, extra)?;
    DEADLINE_NS[slot].store(deadline_ns, Ordering::SeqCst);
    STOPPED[slot].store(false, Ordering::SeqCst);
    SLOT[slot].store(pending.index() + 1, Ordering::SeqCst);
    *call = Some(pending);
    Ok(())
}

/// The call of the process in `slot`: still in flight, or ended now and
/// collected. `stopped` ends it at once (the person's Stop). `None` when it has
/// none.
pub fn poll(slot: usize, stopped: bool) -> Option<Polled> {
    let mut call = CALLS[slot].lock();
    let pending = call.as_ref()?;
    let stopped = stopped || STOPPED[slot].load(Ordering::SeqCst);
    let ended =
        pending.done() || stopped || hpet::nanos() >= DEADLINE_NS[slot].load(Ordering::SeqCst);
    if !ended {
        return Some(Polled::InFlight);
    }
    let pending = call.take()?;
    SLOT[slot].store(0, Ordering::SeqCst);
    drop(call);
    let result = ipc::finish_with(pending);
    Some(Polled::Ended(match result {
        Err(CallError::TimedOut) if stopped => Err(CallError::Cancelled),
        other => other,
    }))
}

/// Whether the process in `slot` has a call in flight that a poll would end
/// now: its reply is in, its deadline passed, or the person pressed Stop.
/// Lock-free, for `svc_recv`'s wait.
pub fn wakes(slot: usize) -> bool {
    match SLOT[slot].load(Ordering::SeqCst) {
        0 => false,
        c => {
            ipc::slot_done(c - 1)
                || super::interrupt_pending(slot)
                || STOPPED[slot].load(Ordering::SeqCst)
                || hpet::nanos() >= DEADLINE_NS[slot].load(Ordering::SeqCst)
        }
    }
}

/// The process in `slot` had a Stop taken by another of its calls: the call it
/// has in flight, if any, ends Interrupted at its next poll.
pub fn stop(slot: usize) {
    if SLOT[slot].load(Ordering::SeqCst) != 0 {
        STOPPED[slot].store(true, Ordering::SeqCst);
    }
}

/// When the call in flight of the process in `slot` runs out, or `u64::MAX`
/// with none: a wait that should end for it ends then.
pub fn deadline(slot: usize) -> u64 {
    match SLOT[slot].load(Ordering::SeqCst) {
        0 => u64::MAX,
        _ => DEADLINE_NS[slot].load(Ordering::SeqCst),
    }
}

/// The process in `slot` exits: its call is dropped, its late reply goes
/// nowhere.
pub fn on_exit(slot: usize) {
    let pending = CALLS[slot].lock().take();
    SLOT[slot].store(0, Ordering::SeqCst);
    if let Some(pending) = pending {
        let _ = ipc::finish_with(pending);
    }
}
