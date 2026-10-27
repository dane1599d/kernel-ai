//! Futexes (L6): FUTEX_WAIT and FUTEX_WAKE on a 32-bit word in the calling process's
//! own memory, so its threads can sleep until another of them changes the word.
//!
//! - A waiter is registered under its tid, keyed by (pid, address): nothing is
//!   shared between processes, and a key outliving its process could not match the
//!   next one in the same slot.
//! - No lost wake-up: the waiter reads the word and registers under the futex lock;
//!   a waker changed the word before its own call, which takes the same lock. Either
//!   the waiter sees the new value and returns at once, or the waker finds it
//!   registered, marks it woken and wakes it; the waiter checks that mark under its
//!   core lock, which `threads::wake` takes too.
//! - Every return takes the lock again and removes the waiter's own registration,
//!   so none outlives the call: a waiter whose registration a waker already removed
//!   was woken and says so, even if its timeout ran out at the same moment.
//! - The word is translated before the lock is taken: SPACES is never taken under a
//!   preempt-off lock. A process's pages are never unmapped while one of its threads
//!   lives, so the frame stays the word's for the whole call.

use super::{identity, killed, uaccess};
use crate::{hpet, memory, threads};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use kabi::Error;
use kabi::layout::USER_END;
use sched::{MAX_THREADS, Tid};
use spin::Mutex;
use x86_64::instructions::interrupts;

/// Each thread's registration while it waits: (pid, word address).
static WAITING: Mutex<[Option<(u64, u64)>; MAX_THREADS]> = Mutex::new([None; MAX_THREADS]);
/// Set by a waker when it removes a thread's registration.
static WOKEN: [AtomicBool; MAX_THREADS] = [const { AtomicBool::new(false) }; MAX_THREADS];
/// Timeouts past this (an hour) mean no timeout, so TimedOut is never early.
const MAX_WAIT_MS: u64 = 3_600_000;

fn with_waiting<R>(f: impl FnOnce(&mut [Option<(u64, u64)>; MAX_THREADS]) -> R) -> R {
    threads::preempt_disable();
    let result = f(&mut WAITING.lock());
    threads::preempt_enable();
    result
}

/// A futex word must be 4-byte aligned and inside user memory.
fn check(addr: u64) -> Result<(), Error> {
    if !addr.is_multiple_of(4) {
        return Err(Error::Invalid);
    }
    if !(crate::memory::user::MIN_USER_ADDRESS..=USER_END - 4).contains(&addr) {
        return Err(Error::Fault);
    }
    Ok(())
}

/// The FUTEX_WAIT call: sleeps while the word at `addr` holds `expected`, until a
/// FUTEX_WAKE for it, `timeout_ms` (0 = no limit) or a kill. Returns 0 when woken,
/// 1 when the word did not hold `expected`; TimedOut, or Dead once killed.
pub fn wait(slot: usize, addr: u64, expected: u64, timeout_ms: u64) -> Result<u64, Error> {
    check(addr)?;
    let key = (identity(slot).pid, addr);
    wait_on(slot, addr, key, expected, timeout_ms)
}

/// Spike S05-1: as [`wait`], keyed by the word's physical address under a pid no
/// process has, so a process waits on a word another one rings.
#[cfg(feature = "spikes")]
pub fn wait_shared(slot: usize, addr: u64, expected: u64, timeout_ms: u64) -> Result<u64, Error> {
    check(addr)?;
    let key = (SHARED, uaccess::phys_of(slot, addr)?);
    wait_on(slot, addr, key, expected, timeout_ms)
}

/// Spike S05-1: as [`wake`], for [`wait_shared`].
#[cfg(feature = "spikes")]
pub fn wake_shared(slot: usize, addr: u64, count: u64) -> Result<u64, Error> {
    check(addr)?;
    Ok(wake_key((SHARED, uaccess::phys_of(slot, addr)?), count))
}

/// The pid [`wait_shared`]'s keys use.
#[cfg(feature = "spikes")]
const SHARED: u64 = u64::MAX;

fn wait_on(
    slot: usize,
    addr: u64,
    key: (u64, u64),
    expected: u64,
    timeout_ms: u64,
) -> Result<u64, Error> {
    let expected = u32::try_from(expected).map_err(|_| Error::Invalid)?;
    let phys = uaccess::phys_of(slot, addr)?;
    let tid = threads::current_tid();
    // SAFETY: `phys` is a frame of this process, which is not freed while this
    // thread lives, reached through the direct map; the word is 4-byte aligned.
    let word = unsafe { &*((memory::hhdm() + phys) as *const AtomicU32) };
    let registered = with_waiting(|waiting| {
        if word.load(Ordering::SeqCst) != expected {
            return false;
        }
        WOKEN[tid].store(false, Ordering::SeqCst);
        waiting[tid] = Some(key);
        true
    });
    if !registered {
        return Ok(1);
    }
    let deadline = match timeout_ms {
        0 => u64::MAX,
        ms if ms > MAX_WAIT_MS => u64::MAX,
        ms => hpet::nanos().saturating_add(ms * 1_000_000),
    };
    let ready = || WOKEN[tid].load(Ordering::SeqCst) || killed(slot);
    while !ready() && hpet::nanos() < deadline {
        if deadline == u64::MAX {
            threads::block_unless(ready);
        } else {
            threads::sleep_until_unless(deadline, ready);
        }
    }
    // Still registered means no waker took this thread: it timed out or was killed.
    let still_waiting = with_waiting(|waiting| waiting[tid].take().is_some());
    match (still_waiting, killed(slot)) {
        (false, _) => Ok(0),
        (true, true) => Err(Error::Dead),
        (true, false) => Err(Error::TimedOut),
    }
}

/// The FUTEX_WAKE call: wakes up to `count` threads of this process waiting on the
/// word at `addr`, and returns how many.
pub fn wake(slot: usize, addr: u64, count: u64) -> Result<u64, Error> {
    check(addr)?;
    Ok(wake_key((identity(slot).pid, addr), count))
}

fn wake_key(key: (u64, u64), count: u64) -> u64 {
    let mut woken: [Option<Tid>; MAX_THREADS] = [None; MAX_THREADS];
    let mut n = 0;
    with_waiting(|waiting| {
        for (tid, entry) in waiting.iter_mut().enumerate() {
            if n as u64 >= count {
                break;
            }
            if *entry == Some(key) {
                *entry = None;
                WOKEN[tid].store(true, Ordering::SeqCst);
                woken[n] = Some(tid);
                n += 1;
            }
        }
    });
    interrupts::without_interrupts(|| threads::wake_all(woken.iter().flatten().copied()));
    n as u64
}

/// Threads of process `pid` waiting in FUTEX_WAIT now, for the tests.
pub fn waiters(pid: u64) -> usize {
    with_waiting(|waiting| {
        waiting
            .iter()
            .filter(|entry| entry.is_some_and(|(owner, _)| owner == pid))
            .count()
    })
}
