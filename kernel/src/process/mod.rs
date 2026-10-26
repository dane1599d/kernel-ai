//! Ring-3 processes, each with its own address space, limits and capabilities, a
//! main thread and, since L6, up to `kabi::thread::MAX_WORKERS` worker threads
//! (`workers.rs`), each a kernel thread.
//!
//! Life of a process: `spawn` fills a table slot and starts a kernel thread running
//! `process_main`, which loads the program and enters ring 3. Every thread leaves
//! through `exit_current` on itself with interrupts on: it moves to the kernel page
//! tables and leaves the process's thread count. The last one out fails the
//! process's IPC calls, frees every user frame, then reports the exit to whoever
//! supervises it.
//!
//! Killing is asynchronous and lock-free: `request_kill` records the first reason in
//! per-slot atomics; `kill`, and any thread leaving a killed process, wakes every
//! thread. Each acts on it when it next returns to ring 3 (a timer tick at most a
//! millisecond later if it is spinning) or when a blocking system call sees it.
//!
//! Lock rules: the TABLE lock is held with preemption off and never across `wake`,
//! a context switch or `memory::with`. A slot's SPACE lock is only taken by the
//! process's own threads, with preemption on, before TABLE and MEMORY.

pub mod burst;
pub mod components;
mod exit;
pub mod futex;
pub mod job;
mod loader;
pub mod programs;
mod space;
pub mod supervision;
pub mod syscall;
pub mod uaccess;
pub mod workers;

pub(crate) use space::lock_space;

pub use burst::blocked;
pub use exit::{exit_current, exit_with};
pub use loader::{grow_heap, link_window};

use crate::entry::TrapFrame;
use crate::gdt::{USER_CODE, USER_DATA};
use crate::memory::user::{MIN_USER_ADDRESS, UserSpace};
use crate::{percpu, threads};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use kabi::layout::{HEAP_BASE, USER_END};
use kabi::thread::MAX_WORKERS;
use kabi::{ExitReason, Limits, MAX_ARGS, MAX_NAME};
use loader::process_main;
use mm::PAGE_SIZE;
use sched::{MAX_THREADS, Tid};
use spin::Mutex;
use workers::Worker;
use x86_64::instructions::interrupts;

pub const MAX_PROCESSES: usize = 16;
/// Most pages one process may be granted: 512 MiB (ADR 0001 D4). The model's
/// holder alone may get `kabi::model::MAX_PAGES_HOLDER`, for its engine.
pub const MAX_PAGES_PER_PROCESS: u64 = 128 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Free,
    Starting,
    Running,
    Exiting,
}

/// A fixed-size process name, so the table never allocates.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Name {
    bytes: [u8; MAX_NAME],
    len: usize,
}

impl Name {
    pub const EMPTY: Name = Name {
        bytes: [0; MAX_NAME],
        len: 0,
    };

    pub fn new(text: &str) -> Option<Name> {
        let len = text.len();
        if len == 0 || len > MAX_NAME {
            return None;
        }
        let mut bytes = [0; MAX_NAME];
        bytes[..len].copy_from_slice(text.as_bytes());
        Some(Name { bytes, len })
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("?")
    }

    pub fn bytes(&self) -> ([u8; MAX_NAME], usize) {
        (self.bytes, self.len)
    }
}

/// How a process faulted, for its exit report.
#[derive(Clone, Copy, Debug)]
pub struct Crash {
    pub vector: u64,
    pub error_code: u64,
    pub address: u64,
    pub rip: u64,
}

#[derive(Clone, Copy)]
pub struct Process {
    pub pid: u64,
    pub state: State,
    /// The service: names the process and its endpoint.
    pub name: Name,
    /// The program it runs: the service name, or a shared host such as `comphost`.
    pub program: Name,
    args: [u8; MAX_ARGS],
    args_len: usize,
    pub limits: Limits,
    /// The main thread.
    pub tid: Option<Tid>,
    /// Worker threads by index (`kabi::thread`).
    pub workers: [Worker; MAX_WORKERS],
    /// Threads counted in the process: the main thread and every worker from the
    /// moment THREAD_SPAWN reserves it. The thread that takes it to 0 tears down.
    pub live_threads: u32,
    /// The address space's PML4, once loaded, for workers to enter.
    pub cr3: u64,
    /// The supervisor itself, which the kernel restarts.
    pub supervisor: bool,
    /// The pid that spawned it (0 for the kernel).
    pub spawner: u64,
    /// Started by JOB_SPAWN: its exit goes to `spawner`, not the supervisor.
    pub job: bool,
    pub heap_end: u64,
    pub crash: Option<Crash>,
}

impl Process {
    const FREE: Process = Process {
        pid: 0,
        state: State::Free,
        name: Name::EMPTY,
        program: Name::EMPTY,
        args: [0; MAX_ARGS],
        args_len: 0,
        limits: Limits {
            max_pages: 0,
            max_burst_ms: 0,
            caps: 0,
            stack_pages: 0,
        },
        tid: None,
        workers: [Worker::FREE; MAX_WORKERS],
        live_threads: 0,
        cr3: 0,
        supervisor: false,
        spawner: 0,
        job: false,
        heap_end: HEAP_BASE,
        crash: None,
    };

    pub fn live(&self) -> bool {
        self.state != State::Free
    }

    pub fn args(&self) -> &[u8] {
        &self.args[..self.args_len]
    }
}

static TABLE: Mutex<[Process; MAX_PROCESSES]> = Mutex::new([Process::FREE; MAX_PROCESSES]);
static SPACES: [Mutex<Option<UserSpace>>; MAX_PROCESSES] =
    [const { Mutex::new(None) }; MAX_PROCESSES];

static NEXT_PID: AtomicU64 = AtomicU64::new(1);
/// Pages all live processes together may be granted; set at start-up.
static PAGE_BUDGET: AtomicU64 = AtomicU64::new(0);

/// Process slot plus one for each thread, or 0 for kernel threads.
static SLOT_OF_TID: [AtomicUsize; MAX_THREADS] = [const { AtomicUsize::new(0) }; MAX_THREADS];
/// Kill reason plus one (0 = none); the first reason wins.
static KILL: [AtomicU8; MAX_PROCESSES] = [const { AtomicU8::new(0) }; MAX_PROCESSES];
static KILL_DETAIL: [AtomicU64; MAX_PROCESSES] = [const { AtomicU64::new(0) }; MAX_PROCESSES];
/// Set when the user presses Ctrl-C for this process; `interrupted` reads and clears it.
static INTERRUPT: [AtomicBool; MAX_PROCESSES] = [const { AtomicBool::new(false) }; MAX_PROCESSES];
/// Set once the process has printed anything: the M10 exit test waits for every
/// `work` process's start-up line before it measures.
static WROTE: [AtomicBool; MAX_PROCESSES] = [const { AtomicBool::new(false) }; MAX_PROCESSES];

/// The process in `slot` printed.
pub fn mark_wrote(slot: usize) {
    WROTE[slot].store(true, Ordering::Relaxed);
}

/// Whether the live process `pid` has printed anything yet.
pub fn has_written(pid: u64) -> bool {
    with_table(|table| table.iter().position(|p| p.live() && p.pid == pid))
        .is_some_and(|slot| WROTE[slot].load(Ordering::Relaxed))
}

/// Runs `f` on the process table with preemption off.
pub fn with_table<R>(f: impl FnOnce(&mut [Process; MAX_PROCESSES]) -> R) -> R {
    threads::preempt_disable();
    let result = f(&mut TABLE.lock());
    threads::preempt_enable();
    result
}

/// Sets how many pages all processes together may hold.
pub fn set_page_budget(pages: u64) {
    PAGE_BUDGET.store(pages, Ordering::Relaxed);
}

fn slot_of(tid: Tid) -> Option<usize> {
    SLOT_OF_TID
        .get(tid)
        .map(|slot| slot.load(Ordering::Acquire))
        .filter(|&slot| slot != 0)
        .map(|slot| slot - 1)
}

/// The calling thread's process slot, if it is a process thread.
pub fn current_slot() -> Option<usize> {
    slot_of(threads::current_tid())
}

/// Whether the process has a Ctrl-C pending, without clearing it.
pub fn interrupt_pending(slot: usize) -> bool {
    INTERRUPT[slot].load(Ordering::SeqCst)
}

/// Reads and clears the process's Ctrl-C flag.
pub fn take_interrupt(slot: usize) -> bool {
    INTERRUPT[slot].swap(false, Ordering::SeqCst)
}

pub fn killed(slot: usize) -> bool {
    KILL[slot].load(Ordering::SeqCst) != 0
}

/// Records why `slot` must die, unless a reason is already recorded. Lock-free, so
/// it works from interrupt handlers. Returns whether this call set the reason.
pub fn request_kill(slot: usize, reason: ExitReason, detail: u64) -> bool {
    let set = KILL[slot]
        .compare_exchange(0, reason as u8 + 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok();
    if set {
        KILL_DETAIL[slot].store(detail, Ordering::SeqCst);
    }
    set
}

/// Kills the process `pid` from another thread and wakes every thread it has.
pub fn kill(pid: u64, reason: ExitReason) -> Result<(), kabi::Error> {
    // The reason is recorded and the threads copied in one critical section: a
    // worker registering later sees the kill itself, and a thread checking for it
    // under its core lock either sees it or is woken.
    let tids = with_table(|table| {
        let slot = table
            .iter()
            .position(|p| p.live() && p.pid == pid && p.state != State::Exiting)?;
        request_kill(slot, reason, 0);
        Some(table[slot].thread_tids())
    });
    workers::wake_all(&tids.ok_or(kabi::Error::NotFound)?);
    Ok(())
}

/// How many threads the live process `pid` has running (0 when it is gone), for
/// the tests.
pub fn thread_count(pid: u64) -> usize {
    with_table(|table| {
        table
            .iter()
            .find(|p| p.live() && p.pid == pid)
            .map_or(0, Process::thread_count)
    })
}

/// RFLAGS bits ring 3 may keep: CF, PF, AF, ZF, SF, TF, DF, OF, RF and AC.
const USER_RFLAGS: u64 = (1 << 0)
    | (1 << 2)
    | (1 << 4)
    | (1 << 6)
    | (1 << 7)
    | (1 << 8)
    | (1 << 10)
    | (1 << 11)
    | (1 << 16)
    | (1 << 18);
const RFLAGS_IF: u64 = 1 << 9;
const RFLAGS_FIXED: u64 = 1 << 1;

/// Last check before `iretq` to ring 3, with interrupts off: acts on a pending kill,
/// and makes sure the frame can only return to ring 3 with IOPL 0.
pub fn return_to_user(frame: &mut TrapFrame) {
    let Some(slot) = current_slot() else {
        // Only process threads ever run in ring 3.
        crate::faults::fatal(frame);
    };
    if killed(slot) {
        die();
    }
    let user = MIN_USER_ADDRESS..USER_END - PAGE_SIZE;
    if !user.contains(&frame.rip) || frame.rsp >= USER_END {
        request_kill(slot, ExitReason::Crashed, 13);
        die();
    }
    frame.cs = u64::from(USER_CODE);
    frame.ss = u64::from(USER_DATA);
    frame.rflags = (frame.rflags & USER_RFLAGS) | RFLAGS_IF | RFLAGS_FIXED;
    debug_assert_eq!(
        percpu::this().preempt_count.load(Ordering::Relaxed),
        0,
        "returning to ring 3 with preemption disabled"
    );
}

/// A CPU exception in ring 3 (or in the return to it): the process dies.
pub fn user_fault(frame: &TrapFrame) -> ! {
    let Some(slot) = current_slot() else {
        crate::faults::fatal(frame);
    };
    let address = if frame.vector == 14 {
        x86_64::registers::control::Cr2::read_raw()
    } else {
        0
    };
    let crash = Crash {
        vector: frame.vector,
        error_code: frame.error_code,
        address,
        rip: frame.rip,
    };
    // Only the fault whose kill counts is reported, when threads fault at once.
    if request_kill(slot, ExitReason::Crashed, frame.vector) {
        with_table(|table| table[slot].crash = Some(crash));
    }
    die()
}

/// Ends the calling process from trap context: interrupts back on, then exit. The
/// thread is on its own kernel stack with only the abandoned trap frame below.
fn die() -> ! {
    interrupts::enable();
    exit_current()
}

/// What `spawn` needs to start a process.
pub struct SpawnRequest<'a> {
    pub name: &'a str,
    /// The program to load.
    pub program: &'a str,
    pub args: &'a [u8],
    pub limits: Limits,
    /// The pid asking (0 for the kernel).
    pub spawner: u64,
    pub supervisor: bool,
    /// The services it gets a route to, with CALL (the plan's `uses=`).
    pub uses: &'a [&'a str],
}

/// Starts a process. At most one live process per service name.
pub fn spawn(request: &SpawnRequest) -> Result<u64, kabi::Error> {
    spawn_with(request, alloc::vec::Vec::new(), false).map_err(|(error, _)| error)
}

/// [`spawn`], with `moved` handles in its table before it runs, and, for `job`, its
/// exit going to its spawner (JOB_SPAWN). On a refusal `moved` comes back.
pub fn spawn_with(
    request: &SpawnRequest,
    moved: alloc::vec::Vec<captable::Entry>,
    job: bool,
) -> Result<u64, (kabi::Error, alloc::vec::Vec<captable::Entry>)> {
    use kabi::Error;
    let checked = (|| {
        if programs::find(request.program).is_none() {
            return Err(Error::NotFound);
        }
        let name = Name::new(request.name).ok_or(Error::Invalid)?;
        let program = Name::new(request.program).ok_or(Error::Invalid)?;
        let max_pages =
            kabi::model::max_pages(request.name, request.program, MAX_PAGES_PER_PROCESS);
        if request.args.len() > MAX_ARGS || request.limits.max_pages > max_pages {
            return Err(Error::TooBig);
        }
        Ok((name, program))
    })();
    let (name, program) = match checked {
        Ok(names) => names,
        Err(error) => return Err((error, moved)),
    };
    let budget = PAGE_BUDGET.load(Ordering::Relaxed);
    let placed = with_table(|table| {
        if table.iter().any(|p| p.live() && p.name == name) {
            return Err(Error::Busy);
        }
        let committed: u64 = table
            .iter()
            .filter(|p| p.live())
            .map(|p| p.limits.max_pages)
            .sum();
        if committed + request.limits.max_pages > budget {
            return Err(Error::NoMemory);
        }
        let slot = table.iter().position(|p| !p.live()).ok_or(Error::Busy)?;
        let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
        let mut process = Process {
            pid,
            state: State::Starting,
            name,
            program,
            limits: request.limits,
            live_threads: 1,
            supervisor: request.supervisor,
            spawner: request.spawner,
            job,
            ..Process::FREE
        };
        process.args[..request.args.len()].copy_from_slice(request.args);
        process.args_len = request.args.len();
        table[slot] = process;
        KILL[slot].store(0, Ordering::SeqCst);
        KILL_DETAIL[slot].store(0, Ordering::SeqCst);
        INTERRUPT[slot].store(false, Ordering::SeqCst);
        WROTE[slot].store(false, Ordering::SeqCst);
        burst::start_process(slot, request.limits.max_burst_ms);
        Ok((slot, pid))
    });
    let (slot, pid) = match placed {
        Ok(placed) => placed,
        Err(error) => return Err((error, moved)),
    };
    // Its startup handles and what was moved to it, before it can run (ADR 0001 D3).
    if let Err(error) = crate::handle::create(slot, request.limits.caps, pid, request.uses) {
        with_table(|table| table[slot] = Process::FREE);
        return Err((error, moved));
    }
    crate::handle::give_at_start(slot, moved);
    if threads::spawn("user", process_main, slot).is_err() {
        crate::handle::destroy(slot);
        with_table(|table| table[slot] = Process::FREE);
        supervision::record_load_failure(pid, &name, request.supervisor, request.spawner);
        return Err((Error::NoMemory, alloc::vec::Vec::new()));
    }
    Ok(pid)
}

/// The calling process's name and argument string.
pub fn identity(slot: usize) -> Process {
    with_table(|table| table[slot])
}

/// The live processes named `name`: their pids, and whether the supervisor
/// started each (a job child, like `scriptrun`, is its parent's, not the
/// supervisor's).
pub fn named(name: &str) -> alloc::vec::Vec<(u64, bool)> {
    let table = with_table(|table| *table);
    table
        .iter()
        .filter(|p| p.live() && p.name.as_str() == name)
        .map(|p| (p.pid, (p.spawner != 0 && !p.job) || p.supervisor))
        .collect()
}

/// Pids and names of live processes, excluding the supervisor.
pub fn services(mut f: impl FnMut(u64, &Name)) {
    let table = with_table(|table| *table);
    for process in table.iter().filter(|p| p.live() && !p.supervisor) {
        f(process.pid, &process.name);
    }
}

/// The process owning each thread slot: its pid, or 0 for kernel threads.
pub fn pids_by_tid() -> [u64; MAX_THREADS] {
    let mut pids = [0; MAX_THREADS];
    let table = with_table(|table| *table);
    for process in table.iter().filter(|p| p.live()) {
        for tid in process.thread_tids().into_iter().flatten() {
            pids[tid] = process.pid;
        }
    }
    pids
}

/// The name of the live process `pid`, and whether it is the supervisor.
pub fn name_of(pid: u64) -> Option<(Name, bool)> {
    with_table(|table| {
        table
            .iter()
            .find(|p| p.live() && p.pid == pid && p.state != State::Exiting)
            .map(|p| (p.name, p.supervisor))
    })
}

/// A thread of process slot `slot` starts: the scheduler applies its label.
pub(super) fn thread_started(tid: Tid, slot: usize) {
    let pid = with_table(|table| table[slot].pid);
    crate::sched_control::on_thread_start(tid, pid);
}

/// Runs `f` on the slot's address space (for copying to and from user memory).
pub fn with_space<R>(slot: usize, f: impl FnOnce(&UserSpace) -> R) -> Option<R> {
    lock_space(slot).as_ref().map(f)
}

/// Whether a live process named `name` holds all the capability `bits`.
pub fn has_cap_by_name(name: &str, bits: u64) -> bool {
    with_table(|table| {
        table
            .iter()
            .any(|p| p.live() && p.name.as_str() == name && p.limits.caps & bits == bits)
    })
}

/// The names of live processes holding all the capability `bits`.
pub fn holders(bits: u64) -> alloc::vec::Vec<alloc::string::String> {
    let table = with_table(|table| *table);
    table
        .iter()
        .filter(|p| p.live() && p.limits.caps & bits == bits)
        .map(|p| alloc::string::String::from(p.name.as_str()))
        .collect()
}
