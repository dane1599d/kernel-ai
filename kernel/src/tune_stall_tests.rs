//! `test=tune-stall workload=A`: a CPU held off its scheduler ticks is not a
//! policy fault. A thread on the last CPU holds preemption off while two
//! threads wait there, which is what made `tune-revert` flaky: the trial's
//! driver reverted that CPU during the hold, and when its ticks resumed the
//! watchdog blamed the restored Interactive for the waits and ejected it.
//! (a) a 300 ms hold with threads waiting ejects nothing;
//! (b) three refusing trials, each with the hold across its candidate window,
//!     end with every CPU running interactive, none on the watchdog's
//!     round-robin.

use crate::sched_control::trial::Candidate;
use crate::tune_tests::{finished, pid_named, refusing, run, running_everywhere};
use crate::{hpet, log, smp, threads};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use kabi::sched::reason;

const MS: u64 = 1_000_000;

/// The waiters spin while this is set.
static SPINNING: AtomicBool = AtomicBool::new(false);
/// The holder sleeps this long, then holds preemption off this long.
static DELAY_MS: AtomicU64 = AtomicU64::new(0);
static HOLD_MS: AtomicU64 = AtomicU64::new(0);
static HELD: AtomicBool = AtomicBool::new(false);

fn spin(_: usize) {
    let mut counter = 0u64;
    while SPINNING.load(Ordering::Relaxed) {
        counter = core::hint::black_box(counter.wrapping_add(1));
    }
}

/// Keeps its CPU from running the scheduler: preemption off, interrupts on.
fn hold(_: usize) {
    threads::sleep_ms(DELAY_MS.load(Ordering::SeqCst));
    let end = hpet::nanos() + HOLD_MS.load(Ordering::SeqCst) * MS;
    threads::preempt_disable();
    while hpet::nanos() < end {
        core::hint::spin_loop();
    }
    threads::preempt_enable();
    HELD.store(true, Ordering::SeqCst);
}

/// Holds `cpu` off after `delay_ms` for `hold_ms`; returns once it is over.
fn start_hold(cpu: usize, delay_ms: u64, hold_ms: u64) -> Result<(), &'static str> {
    DELAY_MS.store(delay_ms, Ordering::SeqCst);
    HOLD_MS.store(hold_ms, Ordering::SeqCst);
    HELD.store(false, Ordering::SeqCst);
    threads::spawn_pinned(cpu, "stall-holder", hold, 0)?;
    Ok(())
}

fn wait_held() -> Result<(), &'static str> {
    for _ in 0..200 {
        if HELD.load(Ordering::SeqCst) {
            return Ok(());
        }
        threads::sleep_ms(20);
    }
    Err("tune-stall: the hold never ended")
}

pub fn check() -> Result<(), &'static str> {
    let editor = pid_named("editor")?;
    let cpu = smp::online() - 1;
    SPINNING.store(true, Ordering::SeqCst);
    for _ in 0..2 {
        threads::spawn_pinned(cpu, "stall-waiter", spin, 0)?;
    }
    threads::sleep_ms(100);

    // (a)
    let ejections = threads::stats().1;
    start_hold(cpu, 0, 300)?;
    wait_held()?;
    threads::sleep_ms(100);
    if threads::stats().1 != ejections || !running_everywhere("interactive") {
        SPINNING.store(false, Ordering::SeqCst);
        return Err("tune-stall: a held-off CPU's policy was ejected");
    }
    log!("tune-stall: (a) a 300 ms hold on cpu {cpu} with threads waiting ejected nothing");

    // (b) The baseline window runs 1 s, then the candidate is flipped in; the
    // hold covers the flip and the revert that follows.
    let mut reasons = Vec::new();
    for _ in 0..3 {
        start_hold(cpu, 900, 500)?;
        let id = run(Candidate::Test(refusing), editor, Vec::new(), 1_000, 2)?;
        let status = finished(id)?;
        wait_held()?;
        threads::sleep_ms(100);
        reasons.push(status.reason);
        if !running_everywhere("interactive") {
            SPINNING.store(false, Ordering::SeqCst);
            return Err("tune-stall: after a held-off revert a CPU is not running interactive");
        }
    }
    SPINNING.store(false, Ordering::SeqCst);
    log!("tune-stall: (b) trial reasons {reasons:?}");
    if reasons.iter().any(|r| *r != reason::EJECTED) {
        return Err("tune-stall: a refusing trial ended for another reason than its ejection");
    }
    log!("tune-stall: (b) three held-off reverts left every CPU on interactive");
    Ok(())
}
