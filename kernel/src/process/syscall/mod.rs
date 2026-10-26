//! System-call dispatch. Every argument is untrusted: sizes are capped before any
//! allocation, times are clamped, pointers are only used through `uaccess`, and
//! capability bits are checked before the call does anything.

mod component;
mod core;
mod device;
mod handle;
mod job;
mod net;
mod sched;
mod service;
mod shm;
mod supervise;

use crate::entry::TrapFrame;
use crate::{cpu, ipc, threads};
use kabi::Error;
use x86_64::instructions::interrupts;

/// Longest a process may wait for another service: the shell's wait for a tune
/// answer, the longest call there is.
const MAX_CALL_MS: u64 = 300_000;
const _: () = assert!(llmproto::tune::CALL_TIMEOUT_MS <= MAX_CALL_MS);
/// A process serving a call must hear back from its own call this long before the
/// call it serves times out.
pub(super) const NESTED_MARGIN_NS: u64 = 1_000_000_000;
/// Longest a process waits for a service that is not up yet.
pub(super) const MAX_BIND_WAIT_NS: u64 = 5_000_000_000;

/// Called by `entry::syscall_entry` with interrupts off.
pub extern "C" fn dispatch(frame: &mut TrapFrame) {
    cpu::clac();
    interrupts::enable();
    let Some(slot) = super::current_slot() else {
        // A kernel thread cannot execute SYSCALL; treat it as fatal.
        interrupts::disable();
        crate::faults::fatal(frame);
    };
    let args = [
        frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9,
    ];
    frame.rax = match handle(slot, frame.rax, args) {
        Ok(value) => value,
        Err(error) => error.to_raw(),
    };
    interrupts::disable();
    super::return_to_user(frame);
}

/// A call that needs capability bits `bits` needs a live handle of each bit's
/// kind and right (ADR 0001 D3): closing the handle drops the right for good.
fn need(slot: usize, bits: u64) -> Result<(), Error> {
    crate::handle::need(slot, bits)
}

/// One area's calls (see the files beside this one).
type Area = fn(usize, u64, &[u64; 6]) -> Option<Result<u64, Error>>;

/// Every area, each in a file of its own: a lane adds its calls to its own area
/// and, for a new area, one line here (ADR 0000 section 4.2).
const AREAS: &[Area] = &[
    core::handle,
    supervise::handle,
    service::handle,
    sched::handle,
    component::handle,
    net::handle,
    handle::handle,
    device::handle,
    shm::handle,
    job::handle,
];

fn handle(slot: usize, number: u64, a: [u64; 6]) -> Result<u64, Error> {
    if super::workers::is_worker(threads::current_tid()) && !super::workers::worker_may(number) {
        return Err(Error::Permission);
    }
    AREAS
        .iter()
        .find_map(|area| area(slot, number, &a))
        .unwrap_or(Err(Error::NotFound))
}

/// What a failed call made on a process's behalf returns to it.
pub(super) fn call_error(slot: usize, error: ipc::CallError) -> Error {
    match error {
        ipc::CallError::Cancelled if super::take_interrupt(slot) => Error::Interrupted,
        ipc::CallError::Cancelled | ipc::CallError::ServiceDied => Error::Dead,
        ipc::CallError::Unavailable => Error::Unavailable,
        ipc::CallError::TimedOut => Error::TimedOut,
        ipc::CallError::Busy => Error::Busy,
        ipc::CallError::NoMemory => Error::NoMemory,
        ipc::CallError::TooBig => Error::TooBig,
    }
}
