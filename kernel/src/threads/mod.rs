//! Kernel threads on every CPU: guarded stacks, context switching, per-CPU
//! scheduler cores, cross-CPU wake-ups, load balancing and reaping.
//!
//! Rules that keep this deadlock-free:
//! - A CPU's `CORES` entry is only locked with interrupts disabled, never two at once,
//!   and is released before switching.
//! - Interrupt handlers acknowledge the APIC before they may switch threads.
//! - Policies never allocate (see the `sched` crate).
//! - A thread is only migrated or reaped once `ON_CPU` says its registers are saved.

mod context;
mod preempt;

pub use context::{enter_address_space, leave_address_space};
pub use preempt::{
    migrations, on_reschedule_ipi, on_tick, preempt_disable, preempt_enable, wake, wake_all,
};

use crate::memory::stack::GuardedStack;
use crate::percpu::{self, MAX_CPUS};
use crate::{apic, hpet, log, smp};
use alloc::boxed::Box;
use context::{initial_frame, switch};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use sched::{Core, EnqueueReason, Interactive, MAX_THREADS, SchedPolicy, Tid};
use spin::Mutex;
use x86_64::instructions::interrupts;

const STACK_PAGES: u64 = 16;
const IDLE_STACK_PAGES: u64 = 4;
const WATCHDOG_NS: u64 = 200_000_000;
/// Ticks between load-balancing attempts on each CPU.
const BALANCE_EVERY: u64 = 4;
/// The boot CPU's idle thread and the boot code itself.
const BOOT_IDLE: Tid = 0;
const BOOT_THREAD: Tid = 1;

static CORES: [Mutex<Option<Core>>; MAX_CPUS] = [const { Mutex::new(None) }; MAX_CPUS];
/// Approximate runnable-plus-running count per CPU, for placement and balancing.
static LOAD: [AtomicUsize; MAX_CPUS] = [const { AtomicUsize::new(0) }; MAX_CPUS];
/// CPU whose core owns each thread.
static OWNER: [AtomicUsize; MAX_THREADS] = [const { AtomicUsize::new(0) }; MAX_THREADS];
/// The CPU plus one a blocked thread asked to move to ([`pin_current`]), or 0.
static MOVE_TO: [AtomicUsize; MAX_THREADS] = [const { AtomicUsize::new(0) }; MAX_THREADS];
/// True from the moment a thread is switched in until its registers are saved again.
static ON_CPU: [AtomicBool; MAX_THREADS] = [const { AtomicBool::new(false) }; MAX_THREADS];
static PINNED: [AtomicBool; MAX_THREADS] = [const { AtomicBool::new(false) }; MAX_THREADS];
static IN_USE: [AtomicBool; MAX_THREADS] = [const { AtomicBool::new(false) }; MAX_THREADS];
/// Each thread slot keeps its guarded stack for reuse after the thread exits.
static STACK_TOP: [AtomicU64; MAX_THREADS] = [const { AtomicU64::new(0) }; MAX_THREADS];
/// Page tables each thread runs on: a process's PML4, or 0 for the kernel's.
static THREAD_CR3: [AtomicU64; MAX_THREADS] = [const { AtomicU64::new(0) }; MAX_THREADS];
/// Set once the boot thread is a scheduled thread.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// True once the scheduler runs.
pub fn running() -> bool {
    RUNNING.load(Ordering::Acquire)
}

/// Saved stack pointer per thread, written by the context switch.
struct SavedStacks(UnsafeCell<[u64; MAX_THREADS]>);

// SAFETY: a slot is only written by the CPU switching that thread out, and only read
// by a CPU switching it in after ON_CPU (release/acquire) shows the write finished.
unsafe impl Sync for SavedStacks {}

static SAVED_RSP: SavedStacks = SavedStacks(UnsafeCell::new([0; MAX_THREADS]));

fn new_policy() -> Box<dyn SchedPolicy> {
    Box::new(Interactive::new())
}

/// Turns the boot code into thread 1 on CPU 0 and creates CPU 0's idle thread.
pub fn init() -> Result<(), &'static str> {
    let idle = GuardedStack::new(IDLE_STACK_PAGES)?;
    let policy = new_policy();
    let idle_rsp = initial_frame(idle.top, idle_loop, 0);
    interrupts::without_interrupts(|| {
        for tid in [BOOT_IDLE, BOOT_THREAD] {
            IN_USE[tid].store(true, Ordering::Relaxed);
            PINNED[tid].store(true, Ordering::Relaxed);
        }
        ON_CPU[BOOT_THREAD].store(true, Ordering::Relaxed);
        // SAFETY: interrupts are off and the scheduler is not running yet.
        unsafe { (*SAVED_RSP.0.get())[BOOT_IDLE] = idle_rsp };
        let mut core = Core::new(policy, BOOT_IDLE, WATCHDOG_NS, hpet::nanos());
        core.adopt_running(BOOT_THREAD, "boot");
        LOAD[0].store(core.load(), Ordering::Relaxed);
        *CORES[0].lock() = Some(core);
        percpu::this().current.store(BOOT_THREAD, Ordering::Relaxed);
    });
    RUNNING.store(true, Ordering::Release);
    log!("threads: scheduler on, policy interactive, watchdog 200 ms");
    Ok(())
}

/// Reserves the thread slot an application processor's boot context will use as
/// its idle thread. Runs on the boot CPU.
pub fn prepare_ap(index: usize) -> Result<Tid, &'static str> {
    let tid = allocate_tid().ok_or("threads: table full")?;
    PINNED[tid].store(true, Ordering::Relaxed);
    OWNER[tid].store(index, Ordering::Relaxed);
    Ok(tid)
}

/// Runs on an application processor: installs its scheduler core, reports it is
/// online, and becomes that CPU's idle thread.
pub fn start_ap(index: usize, idle: Tid, report_online: impl FnOnce()) -> ! {
    let policy = new_policy();
    ON_CPU[idle].store(true, Ordering::Relaxed);
    percpu::this().current.store(idle, Ordering::Relaxed);
    *CORES[index].lock() = Some(Core::new(policy, idle, WATCHDOG_NS, hpet::nanos()));
    report_online();
    idle_loop(0);
    unreachable!("the idle loop returned");
}

fn allocate_tid() -> Option<Tid> {
    (0..MAX_THREADS).find(|&tid| {
        IN_USE[tid]
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    })
}

/// Starts a kernel thread running `entry(arg)` on the least-loaded CPU.
pub fn spawn(name: &'static str, entry: fn(usize), arg: usize) -> Result<Tid, &'static str> {
    spawn_on(None, name, entry, arg)
}

/// Starts a kernel thread that always runs on `cpu`.
pub fn spawn_pinned(
    cpu: usize,
    name: &'static str,
    entry: fn(usize),
    arg: usize,
) -> Result<Tid, &'static str> {
    spawn_on(Some(cpu), name, entry, arg)
}

fn spawn_on(
    cpu: Option<usize>,
    name: &'static str,
    entry: fn(usize),
    arg: usize,
) -> Result<Tid, &'static str> {
    // Cores are published before ONLINE is bumped, so any CPU below online() has one.
    if cpu.is_some_and(|cpu| cpu >= smp::online()) {
        return Err("threads: no such CPU");
    }
    let tid = allocate_tid().ok_or("threads: table full")?;
    let top = match STACK_TOP[tid].load(Ordering::Acquire) {
        0 => match GuardedStack::new(STACK_PAGES) {
            Ok(stack) => {
                STACK_TOP[tid].store(stack.top, Ordering::Release);
                stack.top
            }
            Err(error) => {
                IN_USE[tid].store(false, Ordering::Release);
                return Err(error);
            }
        },
        top => top,
    };
    let rsp = initial_frame(top, entry, arg);
    // A reused slot must not inherit the previous thread's address space.
    THREAD_CR3[tid].store(0, Ordering::Relaxed);
    let target = cpu.unwrap_or_else(least_loaded);
    PINNED[tid].store(cpu.is_some(), Ordering::Relaxed);
    OWNER[tid].store(target, Ordering::Release);
    let kick = interrupts::without_interrupts(|| {
        // SAFETY: interrupts are off; nothing can run this thread before the store.
        unsafe { (*SAVED_RSP.0.get())[tid] = rsp };
        let mut guard = CORES[target].lock();
        let core = guard.as_mut().ok_or("threads: CPU has no scheduler")?;
        core.adopt(tid, name, EnqueueReason::New, hpet::nanos());
        LOAD[target].store(core.load(), Ordering::Relaxed);
        Ok::<bool, &'static str>(core.current() == core.idle())
    });
    let kick = match kick {
        Ok(kick) => kick,
        Err(error) => {
            PINNED[tid].store(false, Ordering::Relaxed);
            IN_USE[tid].store(false, Ordering::Release);
            return Err(error);
        }
    };
    if kick && target != percpu::index() {
        smp::send(target, apic::RESCHEDULE_VECTOR);
    }
    Ok(tid)
}

/// CPUs with a thread running or waiting to run, the caller's own CPU counted only
/// when something besides the caller wants it. The status bar samples it.
pub fn busy_cpus() -> usize {
    let me = percpu::index();
    (0..smp::online())
        .filter(|&cpu| LOAD[cpu].load(Ordering::Relaxed) > usize::from(cpu == me))
        .count()
}

fn least_loaded() -> usize {
    (0..smp::online())
        .min_by_key(|&cpu| LOAD[cpu].load(Ordering::Relaxed))
        .unwrap_or(0)
}

fn idle_loop(_: usize) {
    loop {
        interrupts::enable_and_hlt();
    }
}

/// Logs every thread slot in use that is not running on a CPU now: its owner, its
/// flags and each CPU core's view of it. For a test that lost a thread.
pub fn log_in_use() {
    let cpus = smp::online();
    for tid in 0..MAX_THREADS {
        if !IN_USE[tid].load(Ordering::Acquire) || PINNED[tid].load(Ordering::Relaxed) {
            continue;
        }
        let views: alloc::vec::Vec<alloc::string::String> = (0..cpus)
            .filter_map(|cpu| {
                with_core(cpu, |core, now| match core.state(tid) {
                    sched::State::Free => None,
                    state => Some(alloc::format!(
                        "cpu {cpu}: {} {state:?} (now {now}, current {})",
                        core.name(tid),
                        core.current()
                    )),
                })
                .flatten()
            })
            .collect();
        log!(
            "threads: tid {tid} owner {} on_cpu {} views [{}]",
            OWNER[tid].load(Ordering::Relaxed),
            ON_CPU[tid].load(Ordering::Relaxed),
            views.join("; ")
        );
    }
}

/// Thread slots in use now, for the tests that check none leaked.
pub fn in_use() -> usize {
    IN_USE
        .iter()
        .filter(|used| used.load(Ordering::Acquire))
        .count()
}

/// The calling thread, without taking a lock. Safe with interrupts on: one load, so
/// a migration cannot make it read another CPU's thread (see [`percpu::current`]).
pub fn current_tid() -> Tid {
    percpu::current()
}

/// Runs a state change on this CPU's core and switches to whatever it picks.
fn transition(change: impl FnOnce(&mut Core, u64) -> Tid) {
    interrupts::without_interrupts(|| {
        let me = percpu::index();
        let (from, to) = {
            let mut guard = CORES[me].lock();
            let core = guard.as_mut().expect("threads::init has run");
            let from = core.current();
            let to = change(core, hpet::nanos());
            LOAD[me].store(core.load(), Ordering::Relaxed);
            (from, to)
        };
        switch(from, to);
    });
}

/// Pins the calling thread to `cpu` and moves it there, or unpins it with `None`
/// so load balancing may move it again. A thread moves only once its registers
/// are saved: it blocks with the move asked for, and the CPU it leaves hands it
/// to `cpu` right after the switch ([`move_after_switch`]).
pub fn pin_current(cpu: Option<usize>) -> Result<(), &'static str> {
    let me = current_tid();
    let Some(cpu) = cpu else {
        PINNED[me].store(false, Ordering::Relaxed);
        return Ok(());
    };
    if cpu >= smp::online() {
        return Err("threads: no such CPU");
    }
    PINNED[me].store(true, Ordering::Relaxed);
    while OWNER[me].load(Ordering::Acquire) != cpu {
        MOVE_TO[me].store(cpu + 1, Ordering::Release);
        block_unless(|| false);
    }
    Ok(())
}

/// Right after a switch away from `previous`: if it blocked to move, make it
/// runnable on the CPU it asked for. Interrupts are off.
///
/// A thread preempted after asking for the move but before it blocked keeps its
/// request: it blocks once it runs again, and that switch moves it. Taking the
/// request at the preemption lost it, and the thread then blocked with nothing
/// left to wake it (llm's main thread, hung at start under TCG).
pub(super) fn move_after_switch(previous: Tid) {
    if MOVE_TO[previous].load(Ordering::Acquire) == 0 {
        return;
    }
    let me = percpu::index();
    let now = hpet::nanos();
    let mut target = 0;
    let moved = CORES[me].lock().as_mut().and_then(|core| {
        if core.state(previous) != sched::State::Blocked {
            return None;
        }
        target = MOVE_TO[previous].swap(0, Ordering::AcqRel);
        if target == 0 {
            return None;
        }
        core.wake(previous, now);
        let moved = core.detach_runnable(|tid| tid != previous);
        LOAD[me].store(core.load(), Ordering::Relaxed);
        moved
    });
    let Some((tid, name)) = moved else {
        return;
    };
    let target = target - 1;
    OWNER[tid].store(target, Ordering::Release);
    if let Some(core) = CORES[target].lock().as_mut() {
        core.adopt(tid, name, EnqueueReason::Woken, now);
        LOAD[target].store(core.load(), Ordering::Relaxed);
    }
    if target == me {
        return;
    }
    smp::send(target, apic::RESCHEDULE_VECTOR);
}

/// The CPU the thread `tid` belongs to now.
pub fn owner(tid: Tid) -> usize {
    OWNER[tid].load(Ordering::Acquire)
}

/// Lets another runnable thread on this CPU run first.
pub fn yield_now() {
    transition(|core, now| core.yield_current(now));
}

/// Sleeps at least `ms` milliseconds; an early wake-up just sleeps again. Always
/// gives up the CPU at least once: `sleep_ms(0)` sleeps until the next tick.
pub fn sleep_ms(ms: u64) {
    let deadline = hpet::nanos().saturating_add(ms.saturating_mul(1_000_000));
    transition(|core, now| core.sleep_current(deadline, now));
    while hpet::nanos() < deadline {
        sleep_until_unless(deadline, || false);
    }
}

/// Sleeps until `deadline_ns`, unless `ready()` is already true, or until a `wake`.
/// `ready()` is checked under this CPU's core lock, as in [`block_unless`], so a
/// waker that publishes its condition first and then calls `wake` is never missed.
/// Callers re-check their own conditions: a return says nothing about why.
pub fn sleep_until_unless(deadline_ns: u64, ready: impl Fn() -> bool) {
    interrupts::without_interrupts(|| {
        let me = percpu::index();
        let (from, to) = {
            let mut guard = CORES[me].lock();
            if ready() {
                return;
            }
            let core = guard.as_mut().expect("threads::init has run");
            let now = hpet::nanos();
            if now >= deadline_ns {
                return;
            }
            let switch_to = (core.current(), core.sleep_current(deadline_ns, now));
            LOAD[me].store(core.load(), Ordering::Relaxed);
            switch_to
        };
        switch(from, to);
        crate::process::blocked(from);
    });
}

/// Ends the calling thread.
pub fn exit() -> ! {
    transition(|core, now| core.exit_current(now));
    unreachable!("an exited thread was scheduled again");
}

/// Blocks the calling thread until `wake` is called for it, unless `ready()` is
/// already true. Returns whether it actually blocked.
///
/// `ready()` is checked while holding this CPU's core lock, the same lock `wake`
/// takes. A waker publishes its condition before it locks the core, so either the
/// check sees the condition, or `wake` sees the thread Blocked and makes it runnable.
/// Disabling interrupts alone would only exclude wakers on this CPU. `ready()` must
/// not take a core lock.
pub fn block_unless(ready: impl Fn() -> bool) -> bool {
    interrupts::without_interrupts(|| {
        let me = percpu::index();
        let (from, to) = {
            let mut guard = CORES[me].lock();
            if ready() {
                return false;
            }
            let core = guard.as_mut().expect("threads::init has run");
            let switch_to = (core.current(), core.block_current(hpet::nanos()));
            LOAD[me].store(core.load(), Ordering::Relaxed);
            switch_to
        };
        switch(from, to);
        crate::process::blocked(from);
        true
    })
}

pub fn current() -> Tid {
    interrupts::without_interrupts(|| {
        CORES[percpu::index()]
            .lock()
            .as_ref()
            .map_or(BOOT_THREAD, |core| core.current())
    })
}

/// CPU 0's policy name, plus ejections and context switches summed over all CPUs.
pub fn stats() -> (&'static str, u32, u64) {
    let mut total = ("none", 0, 0);
    for (cpu, slot) in CORES.iter().enumerate().take(smp::online()) {
        interrupts::without_interrupts(|| {
            if let Some(core) = slot.lock().as_ref() {
                if cpu == 0 {
                    total.0 = core.policy_name();
                }
                total.1 += core.ejections();
                total.2 += core.switches();
            }
        });
    }
    total
}

/// The policy installed on CPU 0 (even while the watchdog runs its fallback).
pub fn installed_policy_name() -> &'static str {
    interrupts::without_interrupts(|| {
        CORES[0]
            .lock()
            .as_ref()
            .map_or("none", |core| core.installed_policy_name())
    })
}

/// True if every online CPU has the policy `name` installed (whether or not the
/// watchdog has it on the fallback for now).
pub fn installed_policy_everywhere(name: &str) -> bool {
    CORES.iter().take(smp::online()).all(|slot| {
        interrupts::without_interrupts(|| {
            slot.lock()
                .as_ref()
                .is_some_and(|core| core.installed_policy_name() == name)
        })
    })
}

/// Runs `f` on CPU `cpu`'s scheduler core with interrupts off, given the time.
/// `f` must not take a core lock (see the rules at the top).
pub fn with_core<R>(cpu: usize, f: impl FnOnce(&mut Core, u64) -> R) -> Option<R> {
    if cpu >= smp::online() {
        return None;
    }
    interrupts::without_interrupts(|| {
        let mut guard = CORES[cpu].lock();
        let core = guard.as_mut()?;
        let result = f(core, hpet::nanos());
        LOAD[cpu].store(core.load(), Ordering::Relaxed);
        Some(result)
    })
}

/// Every CPU's Core forgets its ticks and stalls so far (`Core::resync_ticks`).
pub fn resync_ticks() {
    for cpu in 0..smp::online() {
        with_core(cpu, |core, now| core.resync_ticks(now));
    }
}

/// Installs a fresh policy from `make` on every CPU. Old policies are dropped with
/// interrupts enabled.
pub fn set_policy_all(make: fn() -> Box<dyn SchedPolicy>) {
    for slot in CORES.iter().take(smp::online()) {
        let policy = make();
        let old = interrupts::without_interrupts(|| {
            slot.lock().as_mut().map(|core| core.set_policy(policy))
        });
        drop(old);
    }
}
