//! The rule every `ready()` keeps ([`block_unless`](super::block_unless),
//! [`sleep_until_unless`](super::sleep_until_unless), and the `cancel` and
//! `stop` an IPC call checks in its own): it takes no lock that is ever held
//! with interrupts on, so it never allocates either. Atomics are fine, and so is
//! a lock only ever held with interrupts off (an event box's ring, the input
//! queue).
//!
//! `ready()` runs with interrupts off and this CPU's core lock held. A lock held
//! with interrupts on can be held by a thread of this CPU that was switched out,
//! which cannot run again while this CPU spins for it; or by a thread on another
//! CPU whose interrupt handler wakes a thread of this one, and so spins for this
//! CPU's core lock while this CPU spins for its lock. Neither CPU moves again.
//! APP_CTL's `gone` took the IPC lock this way, and JOB_WAIT and JOB_TAKE their
//! tables.
//!
//! Debug builds check it: [`run`] marks this CPU while `ready()` runs, and each
//! helper that takes a lock with interrupts on calls [`not_in_ready`], which
//! counts a lock taken there. A boot case fails when any was ([`ready_locks`]).
//! Release builds check nothing.

#[cfg(debug_assertions)]
use crate::percpu::{self, MAX_CPUS};
#[cfg(debug_assertions)]
use core::sync::atomic::{AtomicPtr, Ordering};

/// `PerCpu::in_ready`: a `ready()` runs, or [`locks_in_ready`]'s probe does.
#[cfg(debug_assertions)]
const READY: u8 = 1;
#[cfg(debug_assertions)]
const PROBE: u8 = 2;

/// The first lock a `ready()` took, for the failure's message.
#[cfg(debug_assertions)]
static FIRST: AtomicPtr<&'static str> = AtomicPtr::new(core::ptr::null_mut());

/// Runs `ready()` for `block_unless` or `sleep_until_unless` on CPU `cpu`, with
/// interrupts off and its core lock held.
#[inline]
pub(super) fn run(cpu: usize, ready: &impl Fn() -> bool) -> bool {
    #[cfg(debug_assertions)]
    {
        let in_ready = &percpu::get(cpu).in_ready;
        let was = in_ready.swap(READY, Ordering::SeqCst);
        let result = ready();
        in_ready.store(was, Ordering::SeqCst);
        result
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = cpu;
        ready()
    }
}

/// Called by each helper that takes `lock` with interrupts on: in a debug build,
/// counts it when the caller is a `ready()`.
#[inline]
pub fn not_in_ready(lock: &'static &'static str) {
    #[cfg(debug_assertions)]
    if percpu::in_ready() {
        // Interrupts are off while `in_ready` is set: the caller is the
        // `ready()` and stays on this CPU.
        let cpu = percpu::this();
        cpu.ready_locks.fetch_add(1, Ordering::SeqCst);
        if cpu.in_ready.load(Ordering::SeqCst) == READY {
            let name = core::ptr::from_ref(lock).cast_mut();
            let _ = FIRST.compare_exchange(
                core::ptr::null_mut(),
                name,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = lock;
}

/// How many locks held with interrupts on `ready()`s took since boot, on all
/// CPUs, and the first one's name; None when none did, and always in a release
/// build.
pub fn ready_locks() -> Option<(u32, &'static str)> {
    #[cfg(debug_assertions)]
    {
        let count = (0..MAX_CPUS)
            .map(|cpu| percpu::get(cpu).ready_locks.load(Ordering::SeqCst))
            .fold(0u32, u32::saturating_add);
        if count == 0 {
            return None;
        }
        let first = FIRST.load(Ordering::SeqCst);
        // SAFETY: FIRST is null or a `&'static &'static str` [`not_in_ready`] stored.
        let name = unsafe { first.as_ref() }.copied().unwrap_or("a lock");
        Some((count, name))
    }
    #[cfg(not(debug_assertions))]
    None
}

/// For the cases that check the rule: runs `f` as a `ready()` runs (this CPU
/// marked, interrupts off, but no core lock held, so a lock it takes cannot
/// freeze it), and returns how many locks held with interrupts on it took. They
/// are not counted in [`ready_locks`]. Always 0 in a release build.
pub fn locks_in_ready(f: impl FnOnce()) -> u32 {
    #[cfg(debug_assertions)]
    {
        x86_64::instructions::interrupts::without_interrupts(|| {
            let cpu = percpu::this();
            let before = cpu.ready_locks.load(Ordering::SeqCst);
            let was = cpu.in_ready.swap(PROBE, Ordering::SeqCst);
            f();
            cpu.in_ready.store(was, Ordering::SeqCst);
            cpu.ready_locks
                .swap(before, Ordering::SeqCst)
                .wrapping_sub(before)
        })
    }
    #[cfg(not(debug_assertions))]
    {
        f();
        0
    }
}
