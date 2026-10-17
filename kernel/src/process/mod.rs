//! Ring-3 processes: one single-threaded process per kernel thread, each with its
//! own address space, limits and capabilities.
//!
//! Life of a process: `spawn` fills a table slot and starts a kernel thread running
//! `process_main`, which loads the program and enters ring 3. Every way out goes
//! through `exit_current` on the process's own thread with interrupts on: it fails
//! the process's IPC calls, moves the thread to the kernel page tables, frees every
//! user frame, then reports the exit to whoever supervises it.
//!
//! Killing is asynchronous and lock-free: `request_kill` records the first reason in
//! per-slot atomics and wakes the thread. The thread acts on it when it next returns
//! to ring 3 (a timer tick at most a millisecond later if it is spinning) or when a
//! blocking system call sees it.
//!
//! Lock rules: the TABLE lock is held with preemption off and never across `wake`,
//! a context switch or `memory::with`. A slot's SPACE lock is only taken by the
//! process's own thread, before MEMORY.

pub mod programs;
pub mod supervision;
pub mod syscall;
pub mod uaccess;

use crate::entry::{self, TrapFrame};
use crate::gdt::{USER_CODE, USER_DATA};
use crate::memory::user::{Access, MIN_USER_ADDRESS, UserSpace};
use crate::{hpet, log, memory, percpu, threads};
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use kabi::layout::{HEAP_BASE, MAX_STACK_PAGES, STACK_PAGES, STACK_TOP, USER_END};
use kabi::{ExitReason, Limits, MAX_ARGS, MAX_NAME};
use mm::PAGE_SIZE;
use sched::{MAX_THREADS, Tid};
use spin::Mutex;
use x86_64::instructions::interrupts;

pub const MAX_PROCESSES: usize = 16;
/// Most pages one process may be granted.
pub const MAX_PAGES_PER_PROCESS: u64 = 16 * 1024;

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
    pub tid: Option<Tid>,
    /// The supervisor itself, which the kernel restarts.
    pub supervisor: bool,
    /// The pid that spawned it (0 for the kernel).
    pub spawner: u64,
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
        supervisor: false,
        spawner: 0,
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
/// CPU time since the process last blocked, and the limit on it.
static BURST_NS: [AtomicU64; MAX_PROCESSES] = [const { AtomicU64::new(0) }; MAX_PROCESSES];
static BURST_LIMIT_NS: [AtomicU64; MAX_PROCESSES] = [const { AtomicU64::new(0) }; MAX_PROCESSES];
/// Set when the user presses Ctrl-C for this process; `interrupted` reads and clears it.
static INTERRUPT: [AtomicBool; MAX_PROCESSES] = [const { AtomicBool::new(false) }; MAX_PROCESSES];

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

/// Kills the process `pid` from another thread and wakes it if it is blocked.
pub fn kill(pid: u64, reason: ExitReason) -> Result<(), kabi::Error> {
    let found = with_table(|table| {
        table
            .iter()
            .position(|p| p.live() && p.pid == pid && p.state != State::Exiting)
            .map(|slot| (slot, table[slot].tid))
    });
    let (slot, tid) = found.ok_or(kabi::Error::NotFound)?;
    request_kill(slot, reason, 0);
    // The reason is published before the wake, so a thread checking it under its
    // core lock either sees it or is woken.
    if let Some(tid) = tid {
        interrupts::without_interrupts(|| threads::wake(tid));
    }
    Ok(())
}

/// Charges CPU time since the last charge on this CPU to `tid`. Interrupts are off
/// (called on every context switch and every tick).
pub fn charge(tid: Tid) {
    let now = hpet::nanos();
    let since = percpu::this().charged_at_ns.swap(now, Ordering::Relaxed);
    let Some(slot) = slot_of(tid) else {
        return;
    };
    let used = now.saturating_sub(since);
    let burst = BURST_NS[slot].fetch_add(used, Ordering::Relaxed) + used;
    let limit = BURST_LIMIT_NS[slot].load(Ordering::Relaxed);
    if limit != 0 && burst > limit {
        request_kill(slot, ExitReason::Runaway, limit / 1_000_000);
    }
}

/// Timer-tick hook: charges the running thread.
pub fn on_tick() {
    charge(threads::current_tid());
}

/// The thread `tid` just came back from blocking or sleeping: its burst starts over.
pub fn blocked(tid: Tid) {
    if let Some(slot) = slot_of(tid) {
        BURST_NS[slot].store(0, Ordering::Relaxed);
    }
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
    with_table(|table| table[slot].crash = Some(crash));
    request_kill(slot, ExitReason::Crashed, frame.vector);
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
}

/// Starts a process. At most one live process per service name.
pub fn spawn(request: &SpawnRequest) -> Result<u64, kabi::Error> {
    use kabi::Error;
    if programs::find(request.program).is_none() {
        return Err(Error::NotFound);
    }
    let name = Name::new(request.name).ok_or(Error::Invalid)?;
    let program = Name::new(request.program).ok_or(Error::Invalid)?;
    if request.args.len() > MAX_ARGS || request.limits.max_pages > MAX_PAGES_PER_PROCESS {
        return Err(Error::TooBig);
    }
    let budget = PAGE_BUDGET.load(Ordering::Relaxed);
    let (slot, pid) = with_table(|table| {
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
            supervisor: request.supervisor,
            spawner: request.spawner,
            ..Process::FREE
        };
        process.args[..request.args.len()].copy_from_slice(request.args);
        process.args_len = request.args.len();
        table[slot] = process;
        KILL[slot].store(0, Ordering::SeqCst);
        KILL_DETAIL[slot].store(0, Ordering::SeqCst);
        INTERRUPT[slot].store(false, Ordering::SeqCst);
        BURST_NS[slot].store(0, Ordering::Relaxed);
        BURST_LIMIT_NS[slot].store(
            request.limits.max_burst_ms.saturating_mul(1_000_000),
            Ordering::Relaxed,
        );
        Ok((slot, pid))
    })?;
    if threads::spawn("user", process_main, slot).is_err() {
        with_table(|table| table[slot] = Process::FREE);
        supervision::record_load_failure(pid, &name, request.supervisor);
        return Err(Error::NoMemory);
    }
    Ok(pid)
}

/// Where a loaded program starts.
#[derive(Clone, Copy)]
struct UserEntry {
    rip: u64,
    rsp: u64,
    cr3: u64,
}

/// The process thread: load the program, then enter ring 3 and never come back here.
fn process_main(slot: usize) {
    let me = threads::current_tid();
    SLOT_OF_TID[me].store(slot + 1, Ordering::Release);
    let (name, program, limits) = with_table(|table| {
        table[slot].tid = Some(me);
        (table[slot].name, table[slot].program, table[slot].limits)
    });
    let entry = match load(slot, program.as_str(), &limits) {
        Ok(entry) => entry,
        Err(error) => {
            log!("process {}: cannot load: {error}", name.as_str());
            if error == QUOTA {
                request_kill(slot, ExitReason::OutOfMemory, limits.max_pages);
            } else {
                request_kill(slot, ExitReason::LoadFailed, 0);
            }
            exit_current();
        }
    };
    // A kill that came before this thread was known could not wake it.
    if killed(slot) {
        exit_current();
    }
    with_table(|table| table[slot].state = State::Running);
    threads::enter_address_space(entry.cr3);
    // SAFETY: the entry point and stack were mapped by `load`, CR3 is loaded, and
    // nothing on this stack needs dropping.
    unsafe { entry::enter_user(entry.rip, entry.rsp) }
}

const QUOTA: &str = "page quota exceeded";

/// Maps a fresh page into the process's address space, within its page quota.
fn map_page(slot: usize, addr: u64, access: Access, max_pages: u64) -> Result<u64, &'static str> {
    let mut guard = SPACES[slot].lock();
    let space = guard.as_mut().ok_or("no address space")?;
    if space.frames() >= max_pages {
        return Err(QUOTA);
    }
    let phys = memory::with(|m| m.map_user_page(space, addr, access))?;
    if space.frames() > max_pages {
        return Err(QUOTA);
    }
    Ok(phys)
}

/// Builds the address space: program segments and the stack.
fn load(slot: usize, name: &str, limits: &Limits) -> Result<UserEntry, &'static str> {
    let program = programs::find(name).ok_or("no such program")?;
    let image = uelf::parse(program.elf, MIN_USER_ADDRESS..HEAP_BASE).map_err(elf_error)?;
    let space = memory::with(|m| m.new_user_space())?;
    let cr3 = space.cr3();
    *SPACES[slot].lock() = Some(space);
    let hhdm = memory::hhdm();
    for segment in image.segments() {
        let access = Access {
            write: segment.write,
            execute: segment.execute,
        };
        for page in segment.pages().step_by(PAGE_SIZE as usize) {
            let phys = map_page(slot, page, access, limits.max_pages)?;
            // File bytes that land in this page, if any.
            let file_start = segment.vaddr;
            let file_end = segment.vaddr + segment.file.len() as u64;
            let from = page.max(file_start);
            let to = (page + PAGE_SIZE).min(file_end);
            if from < to {
                let source = segment.file.start + (from - file_start) as usize;
                let bytes = &program.elf[source..source + (to - from) as usize];
                // SAFETY: the frame was just mapped for this process alone and is
                // reached through the HHDM; the copy stays inside the page.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        (hhdm + phys + (from - page)) as *mut u8,
                        bytes.len(),
                    )
                };
            }
        }
    }
    let stack = Access {
        write: true,
        execute: false,
    };
    let stack_pages = match limits.stack_pages {
        0 => STACK_PAGES,
        pages => pages.min(MAX_STACK_PAGES),
    };
    for page in 1..=stack_pages {
        map_page(slot, STACK_TOP - page * PAGE_SIZE, stack, limits.max_pages)?;
    }
    Ok(UserEntry {
        rip: image.entry,
        // As if `_start` had been called: RSP is 8 below a 16-byte boundary.
        rsp: STACK_TOP - 8,
        cr3,
    })
}

fn elf_error(error: uelf::Error) -> &'static str {
    match error {
        uelf::Error::SegmentOutOfRange => "a segment lies outside user memory",
        uelf::Error::WritableAndExecutable => "a segment is writable and executable",
        uelf::Error::Overlap => "segments overlap",
        uelf::Error::BadEntry => "the entry point is not in executable code",
        uelf::Error::Unsupported => "dynamic linking, an interpreter or TLS",
        _ => "not a valid x86-64 executable",
    }
}

/// Grows the calling process's heap by `bytes` (whole pages). Exceeding the quota
/// kills the process; running the machine out of frames only fails the call.
pub fn grow_heap(slot: usize, bytes: u64) -> Result<u64, kabi::Error> {
    let (end, limits) = with_table(|table| (table[slot].heap_end, table[slot].limits));
    if bytes == 0 {
        return Ok(end);
    }
    let pages = bytes.div_ceil(PAGE_SIZE);
    let new_end = pages
        .checked_mul(PAGE_SIZE)
        .and_then(|grow| end.checked_add(grow))
        .filter(|&new_end| new_end <= HEAP_BASE + kabi::layout::HEAP_MAX)
        .ok_or(kabi::Error::NoMemory)?;
    let heap = Access {
        write: true,
        execute: false,
    };
    let mut mapped_to = end;
    let result: Result<(), &str> =
        (end..new_end)
            .step_by(PAGE_SIZE as usize)
            .try_for_each(|page| {
                map_page(slot, page, heap, limits.max_pages)?;
                mapped_to = page + PAGE_SIZE;
                Ok(())
            });
    with_table(|table| table[slot].heap_end = mapped_to);
    match result {
        Ok(()) => Ok(new_end),
        Err(error) if error == QUOTA => {
            request_kill(slot, ExitReason::OutOfMemory, limits.max_pages);
            Err(kabi::Error::NoMemory)
        }
        Err(error) => {
            log!("process: heap growth failed: {error}");
            Err(kabi::Error::NoMemory)
        }
    }
}

/// Ends the calling process. Runs on its own thread with interrupts on and no locks
/// held; never returns.
pub fn exit_current() -> ! {
    debug_assert!(
        interrupts::are_enabled(),
        "exit_current with interrupts off"
    );
    let tid = threads::current_tid();
    let slot = slot_of(tid).expect("exit_current on a kernel thread");
    let reason =
        ExitReason::from_raw(u64::from(KILL[slot].load(Ordering::SeqCst)).saturating_sub(1))
            .filter(|_| killed(slot))
            .unwrap_or(ExitReason::Killed);
    let detail = KILL_DETAIL[slot].load(Ordering::SeqCst);
    let process = with_table(|table| {
        table[slot].state = State::Exiting;
        table[slot]
    });
    crate::ipc::on_process_exit(process.pid);
    threads::leave_address_space();
    let space = SPACES[slot].lock().take();
    let freed = space.map_or(0, |space| memory::with(|m| m.destroy_user_space(space)));
    report_exit(&process, reason, detail, freed);
    SLOT_OF_TID[tid].store(0, Ordering::Release);
    with_table(|table| table[slot] = Process::FREE);
    supervision::on_exit(&process, reason, detail);
    threads::exit()
}

fn report_exit(process: &Process, reason: ExitReason, detail: u64, freed: u64) {
    let name = process.name.as_str();
    let pid = process.pid;
    match (reason, process.crash) {
        (ExitReason::Crashed, Some(crash)) => {
            let fault = crate::faults::vector_name(crash.vector);
            let source = programs::find(process.program.as_str())
                .and_then(|program| program.describe(crash.rip))
                .unwrap_or_default();
            if crash.vector == 14 {
                log!(
                    "process {name} (pid {pid}) killed: {fault} at {:#x} (code {:#x}), rip {:#x} {source}",
                    crash.address,
                    crash.error_code,
                    crash.rip
                );
            } else {
                log!(
                    "process {name} (pid {pid}) killed: {fault} (code {:#x}), rip {:#x} {source}",
                    crash.error_code,
                    crash.rip
                );
            }
        }
        (ExitReason::Runaway, _) => {
            log!("process {name} (pid {pid}) killed: ran {detail} ms without blocking")
        }
        (ExitReason::OutOfMemory, _) => {
            log!("process {name} (pid {pid}) killed: over its {detail}-page memory quota")
        }
        (ExitReason::Exited, _) => log!("process {name} (pid {pid}) exited with code {detail}"),
        (other, _) => log!("process {name} (pid {pid}) {}", other.name()),
    }
    if freed != 0 {
        log!("process {name} (pid {pid}): {freed} pages freed");
    }
}

/// The `exit` system call.
pub fn exit_with(slot: usize, code: u64) -> ! {
    request_kill(slot, ExitReason::Exited, code);
    exit_current()
}

/// The calling process's name and argument string.
pub fn identity(slot: usize) -> Process {
    with_table(|table| table[slot])
}

/// Pids and names of live processes, excluding the supervisor.
pub fn services(mut f: impl FnMut(u64, &Name)) {
    let table = with_table(|table| *table);
    for process in table.iter().filter(|p| p.live() && !p.supervisor) {
        f(process.pid, &process.name);
    }
}

/// Runs `f` on the slot's address space (for copying to and from user memory).
pub fn with_space<R>(slot: usize, f: impl FnOnce(&UserSpace) -> R) -> Option<R> {
    SPACES[slot].lock().as_ref().map(f)
}

/// Whether a live process named `name` holds all the capability `bits`.
pub fn has_cap_by_name(name: &str, bits: u64) -> bool {
    with_table(|table| {
        table
            .iter()
            .any(|p| p.live() && p.name.as_str() == name && p.limits.caps & bits == bits)
    })
}
