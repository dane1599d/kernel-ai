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

/// Wakes `tid` for a caller on this CPU about to wait for it, pulling it onto
/// this CPU when it sleeps on another: the IPC fast path (the blocking call's
/// server). A wake across CPUs is a reschedule interrupt to a CPU that is often
/// halted, and under KVM each cost about 110-140 us of a call's round trip
/// (`test=ipc-rtt`'s legs, #1000); on this CPU the caller's wait switches
/// straight to it, and its reply wakes the caller here too. Only when this CPU
/// has nothing else to run (its load is the caller alone): a busy CPU would
/// make the server wait behind its work. The move follows load balancing's
/// rule, checked under the owner's lock: never a pinned thread, nor one whose
/// registers are not saved yet (`ON_CPU`); and it counts no wake on the owner
/// (`Core::detach_asleep`). Otherwise, and when it already runs here, a plain
/// [`wake`]. Interrupts must be off.
pub fn wake_here(tid: Tid) {
    let me = percpu::index();
    let owner = OWNER[tid].load(Ordering::Acquire);
    if owner == me || LOAD[me].load(Ordering::Relaxed) > 1 {
        wake(tid);
        return;
    }
    let now = hpet::nanos();
    let moved = CORES[owner].lock().as_mut().and_then(|core| {
        if PINNED[tid].load(Ordering::Relaxed) || ON_CPU[tid].load(Ordering::Acquire) {
            return None;
        }
        let name = core.detach_asleep(tid)?;
        LOAD[owner].store(core.load(), Ordering::Relaxed);
        Some(name)
    });
    let Some(name) = moved else {
        wake(tid);
        return;
    };
    OWNER[tid].store(me, Ordering::Release);
    if let Some(core) = CORES[me].lock().as_mut() {
        core.adopt(tid, name, EnqueueReason::Woken, now);
        LOAD[me].store(core.load(), Ordering::Relaxed);
    }
    HANDOFFS.fetch_add(1, Ordering::Relaxed);
}

static HANDOFFS: AtomicU64 = AtomicU64::new(0);

/// Servers pulled onto their caller's CPU by [`wake_here`] so far.
pub fn handoffs() -> u64 {
    HANDOFFS.load(Ordering::Relaxed)
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

/// Called from the timer interrupt after the APIC is acknowledged.
pub fn on_tick() {
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
    }
    if let Some(to) = next {
        switch(from, to);
    } else if cpu
        .ticks
        .load(Ordering::Relaxed)
        .is_multiple_of(BALANCE_EVERY)
        && let Some((previous, to)) = balance(me)
    {
        switch(previous, to);
    }
}

/// Pulls one runnable thread from the busiest CPU if it has at least two more
/// threads than this one, with the wait it spent there and whether it woke
/// (`Core::detach_waiting`): a woken thread keeps its boost and its
/// wake-to-run sample. Returns the switch to make now, if this CPU was idle or
/// the thread woke and wins the CPU as a wake here would. Locks one core at a
/// time.
fn balance(me: usize) -> Option<(Tid, Tid)> {
    let mine = LOAD[me].load(Ordering::Relaxed);
    let busiest = (0..smp::online())
        .filter(|&cpu| cpu != me)
        .max_by_key(|&cpu| LOAD[cpu].load(Ordering::Relaxed))?;
    if LOAD[busiest].load(Ordering::Relaxed) < mine + 2 {
        return None;
    }
    let now = hpet::nanos();
    let moved = CORES[busiest].lock().as_mut().and_then(|core| {
        let moved = core.detach_waiting(
            |tid| PINNED[tid].load(Ordering::Relaxed) || ON_CPU[tid].load(Ordering::Acquire),
            now,
        );
        LOAD[busiest].store(core.load(), Ordering::Relaxed);
        moved
    })?;
    OWNER[moved.tid].store(me, Ordering::Release);
    let next = CORES[me].lock().as_mut().and_then(|core| {
        let from = core.current();
        core.adopt_detached(moved, now);
        let to = core.preempt_if_pending(now);
        LOAD[me].store(core.load(), Ordering::Relaxed);
        to.map(|to| (from, to))
    });
    MIGRATIONS.fetch_add(1, Ordering::Relaxed);
    next
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
