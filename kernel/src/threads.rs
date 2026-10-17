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
    let cpu = percpu::this();
    cpu.previous.store(from, Ordering::Relaxed);
    ON_CPU[to].store(true, Ordering::Relaxed);
    crate::process::charge(from);
    cpu.current.store(to, Ordering::Relaxed);
    prepare_cpu_for(to);
    let table = SAVED_RSP.0.get();
    // SAFETY: interrupts are off; `to` is runnable here and its registers are saved.
    unsafe { switch_stacks(&raw mut (*table)[from], (*table)[to]) };
    finish_switch();
}

/// Points this CPU at `to`'s address space and kernel stack before switching to it:
/// CR3 (a process's PML4, or the kernel's for kernel threads), the ring-0 stack used
/// on entry from ring 3, the SYSCALL stack, and an empty user GS base so no process
/// sees another's. Interrupts are off.
fn prepare_cpu_for(to: Tid) {
    let cr3 = match THREAD_CR3[to].load(Ordering::Relaxed) {
        0 => crate::memory::kernel_cr3(),
        cr3 => cr3,
    };
    load_cr3(cr3);
    let top = STACK_TOP[to].load(Ordering::Relaxed);
    if top != 0 {
        crate::gdt::set_ring0_stack(top);
        percpu::this().kernel_rsp.store(top, Ordering::Relaxed);
    }
    if THREAD_CR3[to].load(Ordering::Relaxed) != 0 {
        // SAFETY: IA32_KERNEL_GS_BASE holds the inactive (user) GS base while in the
        // kernel; ring 3 gets 0.
        unsafe { x86_64::registers::model_specific::Msr::new(0xC000_0102).write(0) };
    }
}

fn load_cr3(cr3: u64) {
    let current: u64;
    // SAFETY: reading CR3 has no side effects.
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) current, options(nomem, nostack, preserves_flags))
    };
    if current & !0xFFF != cr3 {
        // SAFETY: `cr3` is the kernel PML4 or a process PML4 sharing its kernel half,
        // so the code and stack in use stay mapped.
        unsafe { core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nostack, preserves_flags)) };
    }
}

/// Moves the calling thread onto the process page tables at `cr3`.
pub fn enter_address_space(cr3: u64) {
    interrupts::without_interrupts(|| {
        // THREAD_CR3 first: a switch after this point reloads the same CR3.
        THREAD_CR3[current_tid()].store(cr3, Ordering::Relaxed);
        load_cr3(cr3);
    });
}

/// Moves the calling thread back onto the kernel page tables, after which no CPU
/// has its old process page tables loaded.
pub fn leave_address_space() {
    interrupts::without_interrupts(|| {
        THREAD_CR3[current_tid()].store(0, Ordering::Relaxed);
        load_cr3(crate::memory::kernel_cr3());
    });
}

/// The calling thread, without taking a lock.
pub fn current_tid() -> Tid {
    percpu::this().current.load(Ordering::Relaxed)
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

/// Lets another runnable thread on this CPU run first.
pub fn yield_now() {
    transition(|core, now| core.yield_current(now));
}

/// Sleeps at least `ms` milliseconds; an early wake-up just sleeps again.
pub fn sleep_ms(ms: u64) {
    let deadline = hpet::nanos().saturating_add(ms.saturating_mul(1_000_000));
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
    let next = core.tick(now).or_else(|| {
        if deferred {
            core.preempt_if_pending(now)
        } else {
            None
        }
    });
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
