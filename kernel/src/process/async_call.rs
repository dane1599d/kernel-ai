//! One call in flight per process, not waited for (CALL_SUBMIT, CALL_POLL):
//! the shell sends `act` to `llm` and keeps serving app reads until the reply
//! is in, on one thread (no THREADS, no worker IPC).
//!
//! - The call is an ordinary IPC call ([`ipc::submit_with`]) whose caller does
//!   not wait. Its reply wakes the caller's thread, as any reply does; a thread
//!   blocked in `svc_recv` or `RECV_H` then returns Woken ([`wakes`]), as for an
//!   event box, when no request is queued.
//! - It ends when CALL_POLL collects it: its reply, or why there is none
//!   (timed out, the server died, or the person's Stop). Its deadline passing
//!   also wakes the caller, so a poll answers then.
//! - The person's Stop follows CALL_H's rules (#887): it withdraws the call
//!   while no server has taken it, and a taken call is waited for, so its
//!   change and undo record come back. `llm` sees the Stop through its
//!   caller's flag and answers early. Nothing is sent once the Stop is set.
//! - A process that exits drops its call: the late reply goes nowhere.

use super::MAX_PROCESSES;
use crate::hpet;
use crate::ipc::{self, CallError, Pending};
use alloc::vec::Vec;
use captable::Entry;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

/// Each process's call in flight, each behind a lock of its own: only that
/// process's threads take it.
static CALLS: [Mutex<Option<Pending>>; MAX_PROCESSES] = [const { Mutex::new(None) }; MAX_PROCESSES];
/// Its call slot plus one (0 = none) and its deadline, read without the lock
/// by a wait's wake-up test.
static SLOT: [AtomicUsize; MAX_PROCESSES] = [const { AtomicUsize::new(0) }; MAX_PROCESSES];
static DEADLINE_NS: [AtomicU64; MAX_PROCESSES] = [const { AtomicU64::new(0) }; MAX_PROCESSES];

/// What a poll found.
pub enum Polled {
    /// The call is still in flight.
    InFlight,
    /// The reply, the pid that answered and the handles it moved, or why none.
    Ended(Result<(Vec<u8>, u64, Vec<Entry>), CallError>),
}

/// Sends `request` to `name` for the process in `slot` (pid `pid`), not waited
/// for. Busy while it has one in flight; Cancelled, with nothing sent, once
/// the person pressed Stop.
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
    if super::interrupt_pending(slot) {
        return Err(CallError::Cancelled);
    }
    let pending = ipc::submit_with(name, request, pid, deadline_ns, extra)?;
    DEADLINE_NS[slot].store(deadline_ns, Ordering::SeqCst);
    SLOT[slot].store(pending.index() + 1, Ordering::SeqCst);
    *call = Some(pending);
    Ok(())
}

/// The call of the process in `slot`: still in flight, or ended now and
/// collected. `None` when it has none. With the person's Stop pending, a call
/// no server has taken is withdrawn (Cancelled); a taken one stays in flight
/// until its reply or its deadline.
pub fn poll(slot: usize) -> Option<Polled> {
    let mut call = CALLS[slot].lock();
    let pending = call.as_ref()?;
    let expired = hpet::nanos() >= DEADLINE_NS[slot].load(Ordering::SeqCst);
    if !pending.done() && !expired {
        if !super::interrupt_pending(slot) {
            return Some(Polled::InFlight);
        }
        let pending = call.take()?;
        return match ipc::withdraw(pending) {
            Ok(()) => {
                SLOT[slot].store(0, Ordering::SeqCst);
                Some(Polled::Ended(Err(CallError::Cancelled)))
            }
            Err(taken) => {
                *call = Some(taken);
                Some(Polled::InFlight)
            }
        };
    }
    let pending = call.take()?;
    SLOT[slot].store(0, Ordering::SeqCst);
    drop(call);
    Some(Polled::Ended(ipc::finish_with(pending)))
}

/// Whether the process in `slot` has a call in flight that a poll would end
/// now: its reply is in, its deadline passed, or the person pressed Stop
/// before a server took it. Lock-free, for `svc_recv`'s wait.
pub fn wakes(slot: usize) -> bool {
    match SLOT[slot].load(Ordering::SeqCst) {
        0 => false,
        c => {
            ipc::slot_done(c - 1)
                || (super::interrupt_pending(slot) && ipc::slot_queued(c - 1))
                || hpet::nanos() >= DEADLINE_NS[slot].load(Ordering::SeqCst)
        }
    }
}

/// Whether the process in `slot` has a call in flight.
pub fn in_flight(slot: usize) -> bool {
    SLOT[slot].load(Ordering::SeqCst) != 0
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
