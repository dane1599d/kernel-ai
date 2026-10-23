//! Rescheduling from wake-ups and interrupts: waking a thread on its own CPU, the
//! reschedule interrupt, the timer tick with load balancing, and the preemption
//! count that holds a thread on its CPU.

use super::context::switch;
use super::{BALANCE_EVERY, CORES, LOAD, ON_CPU, OWNER, PINNED};
use crate::{apic, hpet, log, percpu, smp};
use core::sync::atomic::{AtomicU64, Ordering};
use sched::{EnqueueReason, Tid};

/// Makes a blocked or sleeping thread runnable on its own CPU. If it should preempt
/// what that CPU is running, switch here or send that CPU a reschedule interrupt.
/// Interrupts must be off.
pub fn wake(tid: Tid) {
    let owner = OWNER[tid].load(Ordering::Acquire);
    let wants_cpu = CORES[owner].lock().as_mut().is_some_and(|core| {
        let wants = core.wake(tid, hpet::nanos());
        LOAD[owner].store(core.load(), Ordering::Relaxed);
        wants
    });
    if !wants_cpu {
        return;
    }
    if owner == percpu::index() {
        reschedule_here();
    } else {
        smp::send(owner, apic::RESCHEDULE_VECTOR);
    }
}

/// Wakes each of `tids`, those another CPU owns first: waking one this CPU owns
/// may switch to it at once, and the others would wait for the waker to run again.
/// Interrupts must be off.
pub fn wake_all(tids: impl Iterator<Item = Tid> + Clone) {
    let me = percpu::index();
    let mine = |tid: &Tid| OWNER[*tid].load(Ordering::Acquire) == me;
    tids.clone().filter(|tid| !mine(tid)).for_each(wake);
    tids.filter(mine).for_each(wake);
}

/// Reschedule-interrupt handler body.
pub fn on_reschedule_ipi() {
    reschedule_here();
}

fn reschedule_here() {
    let cpu = percpu::this();
    if cpu.preempt_count.load(Ordering::Relaxed) > 0 {
        cpu.need_resched.store(true, Ordering::Relaxed);
        return;
    }
    let me = percpu::index();
    let next = CORES[me].lock().as_mut().map(|core| {
        let from = core.current();
        // A no-op if a tick already ran the woken thread: a late interrupt must not
        // put it back in the queue and cost it its interactive boost.
        let next = core.preempt_if_pending(hpet::nanos());
        LOAD[me].store(core.load(), Ordering::Relaxed);
        (from, next)
    });
    if let Some((from, Some(to))) = next {
        switch(from, to);
    }
}

/// A stall this long is logged: long enough that it is the kernel's own doing
/// (preemption held), not a host's.
const LOGGED_STALL_NS: u64 = 100_000_000;

/// Called from the timer interrupt after the APIC is acknowledged, with where it
/// interrupted (`rip`, `rbp`), so a long stall's log can name what held it.
pub fn on_tick(rip: u64, rbp: u64) {
    let cpu = percpu::this();
    if cpu.preempt_count.load(Ordering::Relaxed) > 0 {
        cpu.need_resched.store(true, Ordering::Relaxed);
        return;
    }
    let me = percpu::index();
    let Some(mut guard) = CORES[me].try_lock() else {
        return;
    };
    let Some(core) = guard.as_mut() else {
        return;
    };
    // Cleared only once the lock is held, so a failed try_lock keeps the request.
    let deferred = cpu.need_resched.swap(false, Ordering::Relaxed);
    let from = core.current();
    let now = hpet::nanos();
    // A wake-up that arrived while preemption was disabled still needs its switch.
    let mut next = core.tick(now).or_else(|| {
        if deferred {
            core.preempt_if_pending(now)
        } else {
            None
        }
    });
    // A trial's guardrail: reverts this CPU at once, then picks again.
    if crate::sched_control::guard::guard(me, core, now) {
        core.preempt(now);
        next = (core.current() != from).then(|| core.current());
    }
    let ejection = core.take_ejection();
    let stall = core.take_stall();
    let policy = core.policy_name();
    LOAD[me].store(core.load(), Ordering::Relaxed);
    drop(guard);
    if let Some(reason) = ejection {
        log!("sched: cpu {me}: policy ejected ({reason}); now running {policy}");
    }
    // Shorter stalls happen when a host does not run the vCPU for a while.
    if let Some(ns) = stall.filter(|ns| *ns >= LOGGED_STALL_NS) {
        log!(
            "sched: cpu {me}: no scheduler tick for {} ms with threads waiting; their waits count from now",
            ns / 1_000_000
        );
        // The tick that ends a hold lands just after whatever held it.
        log!("  at {}", crate::lines::Describe(rip));
        crate::backtrace::print_from(rbp);
    }
    if let Some(to) = next {
        switch(from, to);
    } else if cpu
        .ticks
        .load(Ordering::Relaxed)
        .is_multiple_of(BALANCE_EVERY)
    {
        balance(me);
    }
}

/// Pulls one runnable thread from the busiest CPU if it has at least two more
/// threads than this one. Locks one core at a time.
fn balance(me: usize) {
    let mine = LOAD[me].load(Ordering::Relaxed);
    let busiest = (0..smp::online())
        .filter(|&cpu| cpu != me)
        .max_by_key(|&cpu| LOAD[cpu].load(Ordering::Relaxed));
    let Some(busiest) = busiest else {
        return;
    };
    if LOAD[busiest].load(Ordering::Relaxed) < mine + 2 {
        return;
    }
    let moved = CORES[busiest].lock().as_mut().and_then(|core| {
        let moved = core.detach_runnable(|tid| {
            PINNED[tid].load(Ordering::Relaxed) || ON_CPU[tid].load(Ordering::Acquire)
        });
        LOAD[busiest].store(core.load(), Ordering::Relaxed);
        moved
    });
    let Some((tid, name)) = moved else {
        return;
    };
    OWNER[tid].store(me, Ordering::Release);
    if let Some(core) = CORES[me].lock().as_mut() {
        core.adopt(tid, name, EnqueueReason::Preempted, hpet::nanos());
        LOAD[me].store(core.load(), Ordering::Relaxed);
    }
    MIGRATIONS.fetch_add(1, Ordering::Relaxed);
}

static MIGRATIONS: AtomicU64 = AtomicU64::new(0);

/// Threads moved between CPUs by load balancing so far.
pub fn migrations() -> u64 {
    MIGRATIONS.load(Ordering::Relaxed)
}

/// Keeps the current thread on its CPU until the matching `preempt_enable`.
/// A single GS-relative instruction, so the thread cannot migrate between finding
/// its CPU's counter and updating it.
pub fn preempt_disable() {
    percpu::preempt_count_add(1);
}

pub fn preempt_enable() {
    percpu::preempt_count_add(-1);
}
