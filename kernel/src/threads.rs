//! Kernel threads on every CPU: guarded stacks, context switching, per-CPU
//! scheduler cores, cross-CPU wake-ups, load balancing and reaping.
//!
//! Rules that keep this deadlock-free:
//! - A CPU's `CORES` entry is only locked with interrupts disabled, never two at once,
//!   and is released before switching.
//! - Interrupt handlers acknowledge the APIC before they may switch threads.
//! - Policies never allocate (see the `sched` crate).
//! - A thread is only migrated or reaped once `ON_CPU` says its registers are saved.

use crate::memory::stack::GuardedStack;
use crate::percpu::{self, MAX_CPUS};
use crate::{apic, hpet, log, smp};
use alloc::boxed::Box;
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
/// True from the moment a thread is switched in until its registers are saved again.
static ON_CPU: [AtomicBool; MAX_THREADS] = [const { AtomicBool::new(false) }; MAX_THREADS];
static PINNED: [AtomicBool; MAX_THREADS] = [const { AtomicBool::new(false) }; MAX_THREADS];
static IN_USE: [AtomicBool; MAX_THREADS] = [const { AtomicBool::new(false) }; MAX_THREADS];
/// Each thread slot keeps its guarded stack for reuse after the thread exits.
static STACK_TOP: [AtomicU64; MAX_THREADS] = [const { AtomicU64::new(0) }; MAX_THREADS];

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
    });
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
    })?;
    if kick && target != percpu::index() {
        smp::send(target, apic::RESCHEDULE_VECTOR);
    }
    Ok(tid)
}

fn least_loaded() -> usize {
    (0..smp::online())
        .min_by_key(|&cpu| LOAD[cpu].load(Ordering::Relaxed))
        .unwrap_or(0)
}

/// Builds the stack a new thread starts from: six zeroed callee-saved registers
/// (entry and argument hidden in r12 and r13) and a return into the trampoline.
fn initial_frame(top: u64, entry: fn(usize), arg: usize) -> u64 {
    let frame = (top - 7 * 8) as *mut u64;
    let values = [
        0,
        0,
        arg as u64,
        entry as usize as u64,
        0,
        0,
        trampoline as *const () as u64,
    ];
    for (i, value) in values.iter().enumerate() {
        // SAFETY: the 56 bytes below `top` belong to the thread's stack.
        unsafe { frame.add(i).write(*value) };
    }
    frame as u64
}

/// First code a new thread runs: passes r12 and r13 to `thread_start`.
#[unsafe(naked)]
unsafe extern "C" fn trampoline() -> ! {
    core::arch::naked_asm!(
        "mov rdi, r12",
        "mov rsi, r13",
        "call {start}",
        "ud2",
        start = sym thread_start,
    )
}

extern "C" fn thread_start(entry: usize, arg: usize) -> ! {
    finish_switch();
    interrupts::enable();
    // SAFETY: `entry` was stored from a `fn(usize)` by `initial_frame`.
    let entry: fn(usize) = unsafe { core::mem::transmute(entry) };
    entry(arg);
    exit()
}

fn idle_loop(_: usize) {
    loop {
        interrupts::enable_and_hlt();
    }
}

/// Saves callee-saved registers and RSP for the current thread, loads the next.
///
/// # Safety
/// Interrupts must be disabled and `load_rsp` must come from a previous save or
/// from `initial_frame`.
#[unsafe(naked)]
unsafe extern "C" fn switch_stacks(save_rsp_to: *mut u64, load_rsp: u64) {
    core::arch::naked_asm!(
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov [rdi], rsp",
        "mov rsp, rsi",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "ret",
    )
}

/// Switches this CPU from `from` to `to`. Interrupts must be off and no core locked.
fn switch(from: Tid, to: Tid) {
    if from == to {
        return;
    }
    percpu::this().previous.store(from, Ordering::Relaxed);
    ON_CPU[to].store(true, Ordering::Relaxed);
    let table = SAVED_RSP.0.get();
    // SAFETY: interrupts are off; `to` is runnable here and its registers are saved.
    unsafe { switch_stacks(&raw mut (*table)[from], (*table)[to]) };
    finish_switch();
}

/// Runs on the CPU right after a switch: the previous thread's registers are now
/// saved, so it may migrate, and if it exited its slot can be reused.
fn finish_switch() {
    let previous = percpu::this().previous.load(Ordering::Relaxed);
    ON_CPU[previous].store(false, Ordering::Release);
    let me = percpu::index();
    let released = CORES[me]
        .lock()
        .as_mut()
        .is_some_and(|core| core.release(previous));
    if released {
        PINNED[previous].store(false, Ordering::Relaxed);
        IN_USE[previous].store(false, Ordering::Release);
    }
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

pub fn sleep_ms(ms: u64) {
    transition(|core, now| core.sleep_current(now + ms * 1_000_000, now));
}

/// Ends the calling thread.
pub fn exit() -> ! {
    transition(|core, now| core.exit_current(now));
    unreachable!("an exited thread was scheduled again");
}

/// Blocks the calling thread until `wake` is called for it, unless `ready()` is
/// already true. The check runs with interrupts off, so a wake-up cannot be lost.
pub fn block_unless(ready: impl Fn() -> bool) {
    interrupts::without_interrupts(|| {
        if ready() {
            return;
        }
        let me = percpu::index();
        let (from, to) = {
            let mut guard = CORES[me].lock();
            let core = guard.as_mut().expect("threads::init has run");
            (core.current(), core.block_current(hpet::nanos()))
        };
        switch(from, to);
    });
}

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
        let next = core.preempt(hpet::nanos());
        LOAD[me].store(core.load(), Ordering::Relaxed);
        (from, next)
    });
    if let Some((from, Some(to))) = next {
        switch(from, to);
    }
}

/// Called from the timer interrupt after the APIC is acknowledged.
pub fn on_tick() {
    let cpu = percpu::this();
    if cpu.preempt_count.load(Ordering::Relaxed) > 0 {
        cpu.need_resched.store(true, Ordering::Relaxed);
        return;
    }
    cpu.need_resched.store(false, Ordering::Relaxed);
    let me = percpu::index();
    let Some(mut guard) = CORES[me].try_lock() else {
        return;
    };
    let Some(core) = guard.as_mut() else {
        return;
    };
    let from = core.current();
    let next = core.tick(hpet::nanos());
    let ejection = core.take_ejection();
    let policy = core.policy_name();
    LOAD[me].store(core.load(), Ordering::Relaxed);
    drop(guard);
    if let Some(reason) = ejection {
        log!("sched: cpu {me}: policy ejected ({reason}); now running {policy}");
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
}

/// Keeps the current thread on its CPU until the matching `preempt_enable`.
pub fn preempt_disable() {
    percpu::this().preempt_count.fetch_add(1, Ordering::Relaxed);
}

pub fn preempt_enable() {
    percpu::this().preempt_count.fetch_sub(1, Ordering::Relaxed);
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
