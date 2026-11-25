//! Receive interrupts (P5 plan 3.2): the card's receive queue raises an MSI-X
//! vector, whose handler only notes it and wakes the `net` thread. That thread
//! polls the interface, then moves the frame generation on and wakes every
//! thread waiting for frames ([`wait`]). Without the vector nothing here is
//! used and every waiter sleeps its old fixed gap.
//!
//! A waiter takes a [`ticket`] before it looks at its socket, so a poll that
//! comes between the look and the sleep is never missed: the generation has
//! moved, and it does not sleep.

use crate::threads;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use x86_64::instructions::interrupts;

/// The longest a waiter sleeps with interrupts on: a lost interrupt, or a
/// timer of smoltcp's with no frame to bring it, costs at most this.
const FALLBACK_MS: u64 = 5;
/// Threads waiting at once; past this a waiter sleeps its fixed gap.
const WAITERS: usize = 32;
const NONE: usize = usize::MAX;

/// Whether the receive queue raises its vector.
static ON: AtomicBool = AtomicBool::new(false);
/// An interrupt came since the `net` thread last polled.
static PENDING: AtomicBool = AtomicBool::new(false);
/// The `net` thread, once it runs.
static NET_THREAD: AtomicUsize = AtomicUsize::new(NONE);
/// Moves on after each poll the `net` thread makes.
static GENERATION: AtomicU64 = AtomicU64::new(0);
static WAITING: [AtomicUsize; WAITERS] = [const { AtomicUsize::new(NONE) }; WAITERS];
/// Receive interrupts taken, for the log.
static TAKEN: AtomicU64 = AtomicU64::new(0);

/// The receive queue's MSI-X handler: notes the interrupt and names the `net`
/// thread to wake. Nothing to acknowledge: MSI-X has no ISR read.
pub fn on_interrupt() -> Option<usize> {
    TAKEN.fetch_add(1, Ordering::Relaxed);
    PENDING.store(true, Ordering::Release);
    match NET_THREAD.load(Ordering::Acquire) {
        NONE => None,
        tid => Some(tid),
    }
}

/// Turns the interrupt path on: the card's receive queue has its vector.
pub fn enable() {
    ON.store(true, Ordering::Release);
}

pub fn is_on() -> bool {
    ON.load(Ordering::Acquire)
}

pub fn taken() -> u64 {
    TAKEN.load(Ordering::Relaxed)
}

/// What a waiter holds from before it looked at its socket.
#[derive(Clone, Copy)]
pub struct Ticket(u64);

pub fn ticket() -> Ticket {
    Ticket(GENERATION.load(Ordering::Acquire))
}

/// Sleeps until a poll after `ticket` brought frames in (or up to
/// [`FALLBACK_MS`]) with interrupts on; else `gap_ms`, as before them.
pub fn wait(ticket: Ticket, gap_ms: u64) {
    if !is_on() {
        threads::sleep_ms(gap_ms);
        return;
    }
    let me = threads::current_tid();
    let Some(slot) = WAITING.iter().position(|w| {
        w.compare_exchange(NONE, me, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }) else {
        threads::sleep_ms(gap_ms);
        return;
    };
    let deadline = crate::hpet::nanos().saturating_add(FALLBACK_MS * 1_000_000);
    threads::sleep_until_unless(deadline, || GENERATION.load(Ordering::Acquire) != ticket.0);
    WAITING[slot].store(NONE, Ordering::Release);
}

/// The `net` thread's sleep between polls: `gap_ms`, cut short by a receive
/// interrupt.
pub fn net_thread_sleep(gap_ms: u64) {
    NET_THREAD.store(threads::current_tid(), Ordering::Release);
    let deadline = crate::hpet::nanos().saturating_add(gap_ms.saturating_mul(1_000_000));
    threads::sleep_until_unless(deadline, || PENDING.load(Ordering::Acquire));
}

/// Before the `net` thread polls: interrupts from now on ask for another.
pub fn net_thread_polling() {
    PENDING.store(false, Ordering::Release);
}

/// After the `net` thread polled: the generation moves on and every waiter
/// is woken to look again. Called with no lock held.
pub fn net_thread_polled() {
    if !is_on() {
        return;
    }
    GENERATION.fetch_add(1, Ordering::AcqRel);
    let waiting = || {
        WAITING
            .iter()
            .map(|w| w.load(Ordering::Acquire))
            .filter(|&tid| tid != NONE)
    };
    interrupts::without_interrupts(|| threads::wake_all(waiting()));
}
