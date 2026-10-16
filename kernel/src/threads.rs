//! Kernel threads: guarded stacks, context switching, and the glue between
//! interrupts and the `sched` crate's mechanism and policies.
//!
//! Rules that keep this deadlock-free on one CPU:
//! - `CORE` is only locked with interrupts disabled, and is released before switching.
//! - Interrupt handlers acknowledge the APIC before they may switch threads.
//! - Policies never allocate (see the `sched` crate), so the timer interrupt never
//!   waits on the heap lock a preempted thread might hold.

use crate::memory::stack::GuardedStack;
use crate::{hpet, log};
use alloc::boxed::Box;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use sched::{Core, IDLE, Interactive, MAX_THREADS, SchedPolicy, Tid};
use spin::Mutex;
use x86_64::instructions::interrupts;

const STACK_PAGES: u64 = 16;
const IDLE_STACK_PAGES: u64 = 4;
const WATCHDOG_NS: u64 = 200_000_000;

static CORE: Mutex<Option<Core>> = Mutex::new(None);

/// Saved stack pointer per thread, written by the context switch.
struct SavedStacks(UnsafeCell<[u64; MAX_THREADS]>);

// SAFETY: only touched with interrupts disabled on a single CPU.
unsafe impl Sync for SavedStacks {}

static SAVED_RSP: SavedStacks = SavedStacks(UnsafeCell::new([0; MAX_THREADS]));
static PREEMPT_COUNT: AtomicU32 = AtomicU32::new(0);
static NEED_RESCHED: AtomicBool = AtomicBool::new(false);

/// Turns the running boot code into thread 1 and creates the idle thread.
pub fn init() -> Result<(), &'static str> {
    let idle = GuardedStack::new(IDLE_STACK_PAGES)?;
    let policy: Box<dyn SchedPolicy> = Box::new(Interactive::new());
    let idle_rsp = initial_frame(idle.top, idle_loop, 0);
    interrupts::without_interrupts(|| {
        // SAFETY: interrupts are off and the scheduler is not running yet.
        unsafe { (*SAVED_RSP.0.get())[IDLE] = idle_rsp };
        *CORE.lock() = Some(Core::new(policy, "boot", WATCHDOG_NS, hpet::nanos()));
    });
    log!("threads: scheduler on, policy interactive, watchdog 200 ms");
    Ok(())
}

/// Starts a kernel thread running `entry(arg)`.
pub fn spawn(name: &'static str, entry: fn(usize), arg: usize) -> Result<Tid, &'static str> {
    let stack = GuardedStack::new(STACK_PAGES)?;
    let rsp = initial_frame(stack.top, entry, arg);
    interrupts::without_interrupts(|| {
        let mut guard = CORE.lock();
        let core = guard.as_mut().ok_or("threads: not initialized")?;
        let tid = core
            .spawn(name, hpet::nanos())
            .ok_or("threads: table full")?;
        // SAFETY: interrupts are off; the thread cannot run before this store.
        unsafe { (*SAVED_RSP.0.get())[tid] = rsp };
        Ok(tid)
    })
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
        // SAFETY: the 56 bytes below `top` belong to the freshly mapped stack.
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

/// Switches from `from` to `to`. Interrupts must be off and `CORE` unlocked.
fn switch(from: Tid, to: Tid) {
    if from == to {
        return;
    }
    let table = SAVED_RSP.0.get();
    // SAFETY: interrupts are off; both slots belong to live threads.
    unsafe { switch_stacks(&raw mut (*table)[from], (*table)[to]) };
}

/// Runs a state change on the core and switches to whatever it picks.
fn transition(change: impl FnOnce(&mut Core, u64) -> Tid) {
    interrupts::without_interrupts(|| {
        let (from, to) = {
            let mut guard = CORE.lock();
            let core = guard.as_mut().expect("threads::init has run");
            let from = core.current();
            (from, change(core, hpet::nanos()))
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
        let (from, to) = {
            let mut guard = CORE.lock();
            let core = guard.as_mut().expect("threads::init has run");
            (core.current(), core.block_current(hpet::nanos()))
        };
        switch(from, to);
    });
}

/// Makes a blocked thread runnable; may switch to it at once. Interrupts must be off.
pub fn wake(tid: Tid) {
    let Some(mut guard) = CORE.try_lock() else {
        return;
    };
    let Some(core) = guard.as_mut() else {
        return;
    };
    let from = core.current();
    let next = core.wake(tid, hpet::nanos());
    drop(guard);
    if PREEMPT_COUNT.load(Ordering::Relaxed) > 0 {
        NEED_RESCHED.store(true, Ordering::Relaxed);
        return;
    }
    if let Some(to) = next {
        switch(from, to);
    }
}

/// Called from the timer interrupt after the APIC is acknowledged.
pub fn on_tick() {
    if PREEMPT_COUNT.load(Ordering::Relaxed) > 0 {
        NEED_RESCHED.store(true, Ordering::Relaxed);
        return;
    }
    NEED_RESCHED.store(false, Ordering::Relaxed);
    let Some(mut guard) = CORE.try_lock() else {
        return;
    };
    let Some(core) = guard.as_mut() else {
        return;
    };
    let from = core.current();
    let next = core.tick(hpet::nanos());
    let ejection = core.take_ejection();
    let policy = core.policy_name();
    drop(guard);
    if let Some(reason) = ejection {
        log!("sched: policy ejected ({reason}); now running {policy}");
    }
    if let Some(to) = next {
        switch(from, to);
    }
}

/// Keeps the current thread on the CPU until the matching `preempt_enable`.
pub fn preempt_disable() {
    PREEMPT_COUNT.fetch_add(1, Ordering::Relaxed);
}

pub fn preempt_enable() {
    PREEMPT_COUNT.fetch_sub(1, Ordering::Relaxed);
}

pub fn current() -> Tid {
    interrupts::without_interrupts(|| CORE.lock().as_ref().map_or(1, |core| core.current()))
}

/// Active policy name, ejection count and context-switch count.
pub fn stats() -> (&'static str, u32, u64) {
    interrupts::without_interrupts(|| {
        CORE.lock().as_ref().map_or(("none", 0, 0), |core| {
            (core.policy_name(), core.ejections(), core.switches())
        })
    })
}

/// Installs a scheduling policy. The old one is dropped with interrupts enabled.
pub fn set_policy(policy: Box<dyn SchedPolicy>) {
    let old =
        interrupts::without_interrupts(|| CORE.lock().as_mut().map(|core| core.set_policy(policy)));
    drop(old);
}
