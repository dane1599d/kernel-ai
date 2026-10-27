//! What the kernel does for supervision.
//!
//! - The init thread starts the ring-3 supervisor and restarts it whenever it dies.
//! - Exits of the services the supervisor started queue here, not in the supervisor,
//!   so none is lost while it is down. A record handed out by `wait_child` stays at
//!   the head until the next `wait_child` call (which acknowledges it); if the
//!   supervisor dies first, its successor gets the record again.
//! - The supervisor's saved state lives in a kernel buffer that outlives it.
//! - Exits of processes the kernel started itself (self-tests) are kept briefly, so
//!   the test that started one can see how it ended.

use super::uaccess::{copy_from_user, copy_to_user, read_struct};
use super::{Name, Process, SpawnRequest, spawn};
use crate::{hpet, log, threads};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use kabi::{Error, ExitReason, ExitRecord, Limits, MAX_ARGS, MAX_NAME, MAX_STATE, cap};
use spin::{Mutex, Once};

const MAX_RECORDS: usize = 32;
/// A supervisor that dies this many times in a row before reaching its main loop
/// gets its saved state cleared, in case the state is what kills it.
const EARLY_DEATHS_BEFORE_RESET: u32 = 3;
const SUPERVISOR_LIMITS: Limits = Limits {
    max_pages: 2048,
    max_burst_ms: 2000,
    // SERVE for the `stop` and `start` requests it takes between exits.
    caps: cap::SUPERVISE | cap::SERVE,
    stack_pages: 0,
};
/// Limits for a service spawned without its own.
const DEFAULT_PAGES: u64 = 1024;
const DEFAULT_BURST_MS: u64 = 2000;
/// The one program that may hold `cap::COMPONENT`.
pub const COMPONENT_HOST: &str = "comphost";
/// The one service that may hold `cap::STOP`.
pub const STOPPER: &str = "shell";
/// The service, running the program of the same name, that may hold the screen
/// and read the input queue (ADR 0005 §2).
pub const DESKTOP: &str = "desktop";
/// The terminal window, the only reader of the console's rows (P3 task 8).
pub const TERM: &str = "term";
/// The service, running the program of the same name, that may hold the slots
/// and the boot record (ADR 0008 §10).
pub const UPDATED: &str = "updated";
/// The one service that may hold `cap::SCRIPT`.
pub const SCRIPTER: &str = "shell";
/// The one service, and program, that may hold `cap::PUBLISH`.
pub const PUBLISHER: &str = "edit";
/// The one service, and program, that may hold `cap::PARTITION`.
pub const FS: &str = "fs";
/// The one service, and program, that may hold `cap::SYSTEM_STATE`.
pub const STATE_HOLDER: &str = "shell";
/// Whether the service `name` running `program` may have the storage bits in
/// `caps`: the `kai-data` partition is the file system service's alone (ADR
/// 0004 §7), and `/system/state` the shell's (P2 task 12).
pub fn storage_caps_allowed(name: &str, program: &str, caps: u64) -> bool {
    let only = |bit: u64, holder: &str| caps & bit == 0 || (name == holder && program == holder);
    only(cap::PARTITION, FS) && only(cap::SYSTEM_STATE, STATE_HOLDER)
}

/// Longest any supervision call may block.
const MAX_WAIT_MS: u64 = 60_000;

struct ExitQueue {
    records: [ExitRecord; MAX_RECORDS],
    head: usize,
    len: usize,
    /// The head was handed out and not yet acknowledged.
    head_delivered: bool,
    dropped: u64,
}

static QUEUE: Mutex<ExitQueue> = Mutex::new(ExitQueue {
    records: [ExitRecord::empty(); MAX_RECORDS],
    head: 0,
    len: 0,
    head_delivered: false,
    dropped: 0,
});
/// Queue length, readable without the lock (wake-up conditions).
static QUEUED: AtomicUsize = AtomicUsize::new(0);
/// Thread id plus one of the supervisor blocked in `wait_child`.
static WAITER: AtomicUsize = AtomicUsize::new(0);
static SUPERVISOR_EXITS: AtomicU64 = AtomicU64::new(0);
static SUPERVISOR_PID: AtomicU64 = AtomicU64::new(0);
/// The current supervisor has called `wait_child` at least once.
static REACHED_LOOP: AtomicBool = AtomicBool::new(false);
static INIT_TID: AtomicUsize = AtomicUsize::new(0);

struct SavedState {
    bytes: Vec<u8>,
    saves: u64,
}

static STATE: Mutex<SavedState> = Mutex::new(SavedState {
    bytes: Vec::new(),
    saves: 0,
});
static PLAN: Once<String> = Once::new();

/// How many exits of processes the kernel started are kept, oldest overwritten first.
const KERNEL_EXITS: usize = 32;

#[derive(Clone, Copy)]
struct KernelExit {
    pid: u64,
    reason: ExitReason,
    detail: u64,
}

/// The latest exits of processes the kernel started, and the next one to overwrite.
struct KernelExits {
    exits: [Option<KernelExit>; KERNEL_EXITS],
    next: usize,
}

static KERNEL_CHILDREN: Mutex<KernelExits> = Mutex::new(KernelExits {
    exits: [None; KERNEL_EXITS],
    next: 0,
});

/// A process whose thread could not start. A service the supervisor started learns
/// that from SPAWN's error, so only the supervisor and the kernel's own children
/// are reported.
pub(super) fn record_load_failure(pid: u64, name: &Name, supervisor: bool, spawner: u64) {
    if spawner != 0 {
        return;
    }
    let process = Process {
        pid,
        name: *name,
        supervisor,
        ..Process::FREE
    };
    on_exit(&process, ExitReason::LoadFailed, 0);
}

/// Reports an ended process: to the init thread if it was the supervisor, to the
/// exit queue if the supervisor started it, and to [`kernel_child_exit`] if the
/// kernel did.
pub(super) fn on_exit(process: &Process, reason: ExitReason, detail: u64) {
    if process.job {
        super::job::on_exit(process, reason, detail);
        return;
    }
    let wake = if process.supervisor {
        // A record it took but never acknowledged goes to its successor.
        with_queue(|queue| queue.head_delivered = false);
        SUPERVISOR_EXITS.fetch_add(1, Ordering::SeqCst);
        INIT_TID.load(Ordering::SeqCst)
    } else if process.spawner != 0 {
        let (name, len) = process.name.bytes();
        let record = ExitRecord {
            pid: process.pid,
            reason: reason as u64,
            detail,
            name_len: len as u64,
            name,
        };
        let dropped = with_queue(|queue| {
            let dropped = queue.len == MAX_RECORDS;
            if dropped {
                queue.head = (queue.head + 1) % MAX_RECORDS;
                queue.len -= 1;
                queue.head_delivered = false;
                queue.dropped += 1;
            }
            let tail = (queue.head + queue.len) % MAX_RECORDS;
            queue.records[tail] = record;
            queue.len += 1;
            QUEUED.store(queue.len, Ordering::SeqCst);
            dropped
        });
        if dropped {
            log!("supervision: exit queue full; dropped the oldest record");
        }
        WAITER.load(Ordering::SeqCst)
    } else {
        threads::preempt_disable();
        {
            let mut children = KERNEL_CHILDREN.lock();
            let next = children.next;
            children.exits[next] = Some(KernelExit {
                pid: process.pid,
                reason,
                detail,
            });
            children.next = (next + 1) % KERNEL_EXITS;
        }
        threads::preempt_enable();
        0
    };
    if wake != 0 {
        x86_64::instructions::interrupts::without_interrupts(|| threads::wake(wake - 1));
    }
}

/// How a process the kernel started ended, once it has: the reason and its detail
/// (the exit code, or the vector that stopped it).
pub fn kernel_child_exit(pid: u64) -> Option<(ExitReason, u64)> {
    threads::preempt_disable();
    let found = KERNEL_CHILDREN
        .lock()
        .exits
        .iter()
        .flatten()
        .find(|exit| exit.pid == pid)
        .map(|exit| (exit.reason, exit.detail));
    threads::preempt_enable();
    found
}

fn with_queue<R>(f: impl FnOnce(&mut ExitQueue) -> R) -> R {
    threads::preempt_disable();
    let result = f(&mut QUEUE.lock());
    threads::preempt_enable();
    result
}

/// The `wait_child` system call. A caller that serves (the supervisor) also
/// returns 2 when a request waits for its service, which it then takes with
/// `svc_recv`.
pub fn wait_child(slot: usize, out: u64, timeout_ms: u64) -> Result<u64, Error> {
    REACHED_LOOP.store(true, Ordering::SeqCst);
    let deadline = hpet::nanos().saturating_add(timeout_ms.min(MAX_WAIT_MS) * 1_000_000);
    let me = threads::current_tid();
    let serves = crate::handle::need(slot, cap::SERVE).is_ok();
    loop {
        let record = with_queue(|queue| {
            if queue.head_delivered {
                // The previous record is acknowledged by this call.
                queue.head = (queue.head + 1) % MAX_RECORDS;
                queue.len -= 1;
                queue.head_delivered = false;
                QUEUED.store(queue.len, Ordering::SeqCst);
            }
            (queue.len > 0).then(|| {
                queue.head_delivered = true;
                queue.records[queue.head]
            })
        });
        if let Some(record) = record {
            // SAFETY: ExitRecord is plain old data.
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    (&raw const record).cast::<u8>(),
                    core::mem::size_of::<ExitRecord>(),
                )
            };
            copy_to_user(slot, out, bytes)?;
            return Ok(1);
        }
        if super::killed(slot) {
            return Err(Error::Dead);
        }
        // Bound as `svc_recv` binds it, and watched, so a request wakes the wait.
        let endpoint = match serves {
            true => Some(crate::ipc::watch(slot, me)?),
            false => None,
        };
        let request = move || endpoint.is_some_and(crate::ipc::pending);
        if request() {
            endpoint.inspect(|&e| crate::ipc::unwatch(e, me));
            return Ok(2);
        }
        if hpet::nanos() >= deadline {
            endpoint.inspect(|&e| crate::ipc::unwatch(e, me));
            return Ok(0);
        }
        WAITER.store(me + 1, Ordering::SeqCst);
        threads::sleep_until_unless(deadline, || {
            QUEUED.load(Ordering::SeqCst) > 0 || super::killed(slot) || request()
        });
        WAITER.store(0, Ordering::SeqCst);
        endpoint.inspect(|&e| crate::ipc::unwatch(e, me));
    }
}

pub fn supervisor_pid() -> u64 {
    SUPERVISOR_PID.load(Ordering::SeqCst)
}

/// The `spawn` system call: the supervisor starts a service.
pub fn spawn_service(slot: usize, request_ptr: u64) -> Result<u64, Error> {
    let request: kabi::SpawnRequest = read_struct(slot, request_ptr)?;
    let name_len = usize::try_from(request.name_len).map_err(|_| Error::Invalid)?;
    let args_len = usize::try_from(request.args_len).map_err(|_| Error::Invalid)?;
    if name_len == 0 || name_len > MAX_NAME || args_len > MAX_ARGS {
        return Err(Error::TooBig);
    }
    let mut name = [0u8; MAX_NAME];
    copy_from_user(slot, request.name_ptr, &mut name[..name_len])?;
    let mut args = [0u8; MAX_ARGS];
    copy_from_user(slot, request.args_ptr, &mut args[..args_len])?;
    let name = core::str::from_utf8(&name[..name_len]).map_err(|_| Error::Invalid)?;
    let program_len = usize::try_from(request.program_len).map_err(|_| Error::Invalid)?;
    if program_len > MAX_NAME {
        return Err(Error::TooBig);
    }
    let mut program = [0u8; MAX_NAME];
    copy_from_user(slot, request.program_ptr, &mut program[..program_len])?;
    let program = match program_len {
        0 => name,
        _ => core::str::from_utf8(&program[..program_len]).map_err(|_| Error::Invalid)?,
    };
    if name == "supervisor" || program == "supervisor" || request.limits.caps & !cap::GRANTABLE != 0
    {
        return Err(Error::Permission);
    }
    // Component versions and the ledger are only for the host that runs them.
    if request.limits.caps & cap::COMPONENT != 0 && program != COMPONENT_HOST {
        return Err(Error::Permission);
    }
    // Stopping processes is for the shell, whose rows ask first.
    if request.limits.caps & cap::STOP != 0 && name != STOPPER {
        return Err(Error::Permission);
    }
    // Starting `scriptrun` is for the agent core (ADR 0001 D6).
    if request.limits.caps & cap::SCRIPT != 0 && name != SCRIPTER {
        return Err(Error::Permission);
    }
    // Uploading versions and reaching the build service are for the edit service.
    if request.limits.caps & cap::PUBLISH != 0 && (name != PUBLISHER || program != PUBLISHER) {
        return Err(Error::Permission);
    }
    // The screen and the input queue are `desktop`'s alone (ADR 0005 §2).
    if request.limits.caps & (cap::DISPLAY | cap::INPUT) != 0
        && (name != DESKTOP || program != DESKTOP)
    {
        return Err(Error::Permission);
    }
    // The console's rows are the terminal window's alone.
    if request.limits.caps & cap::TERM != 0 && (name != TERM || program != TERM) {
        return Err(Error::Permission);
    }
    // The slots and the boot record are `updated`'s alone (ADR 0008 §10).
    if request.limits.caps & cap::UPDATE != 0 && (name != UPDATED || program != UPDATED) {
        return Err(Error::Permission);
    }
    // The model is mapped, and worker threads run, only in the llm service.
    let holder = crate::model::HOLDER;
    if request.limits.caps & (cap::MODEL | cap::THREADS) != 0
        && (name != holder || program != holder)
    {
        return Err(Error::Permission);
    }
    if !storage_caps_allowed(name, program, request.limits.caps) {
        return Err(Error::Permission);
    }
    let limits = Limits {
        max_pages: match request.limits.max_pages {
            0 => DEFAULT_PAGES,
            pages => pages,
        },
        max_burst_ms: match request.limits.max_burst_ms {
            0 => DEFAULT_BURST_MS,
            ms => ms.max(10),
        },
        caps: request.limits.caps,
        stack_pages: request
            .limits
            .stack_pages
            .min(kabi::layout::MAX_STACK_PAGES),
    };
    let me = super::identity(slot).pid;
    // Its routes, folders and boot files come from the kernel's own copy of the
    // plan, never the request: the plan line is the authority (ADR 0011 §12).
    // Its folders and boot files go only to the program the line names.
    let grants = plan_grants(name, program).map_err(|why| {
        log!("supervision: {why}");
        Error::Permission
    })?;
    let uses: Vec<&str> = grants.uses.iter().map(String::as_str).collect();
    let request = SpawnRequest {
        name,
        program,
        args: &args[..args_len],
        limits,
        spawner: me,
        supervisor: false,
        uses: &uses,
    };
    let pid = super::spawn_in(&request, Vec::new(), false, &grants).map_err(|(error, _)| error)?;
    if program == name {
        log!("process {name} (pid {pid}) started by the supervisor");
    } else {
        log!("process {name} (pid {pid}, program {program}) started by the supervisor");
    }
    Ok(pid)
}

/// The `proc_stop` system call: stops every live process named like the request
/// that the kernel started rather than the supervisor, and returns how many went.
/// The shell, the supervisor and llm, and the caller itself, are never stopped; a
/// service is stopped through the supervisor, which then keeps it down.
pub fn proc_stop(slot: usize, ptr: u64, len: u64) -> Result<u64, Error> {
    let mut name = [0u8; MAX_NAME];
    let name = name_arg(slot, ptr, len, &mut name)?;
    stop_named(super::identity(slot).name.as_str(), name)
}

/// The `proc_stop_once` system call.
pub fn proc_stop_once(slot: usize, ptr: u64, len: u64, badge: u64, id: u64) -> Result<u64, Error> {
    let mut name = [0u8; MAX_NAME];
    let name = name_arg(slot, ptr, len, &mut name)?;
    stop_once(slot, name, badge, id)
}

/// [`stop_named`] for the process in `slot`, once the `ONCE` it holds under
/// `badge` checks against menu `id`'s `process_stop("<name>")`, which uses it up;
/// with `badge` 0, once a standing rule it holds covers that call.
pub fn stop_once(slot: usize, name: &str, badge: u64, id: u64) -> Result<u64, Error> {
    let call = termcore::once::call_text("process_stop", &[termcore::once::CallArg::Text(name)])
        .ok_or(Error::Invalid)?;
    if badge == 0 {
        if !crate::handle::rules::covers(slot, &call) {
            return Err(Error::Permission);
        }
    } else {
        if !crate::handle::grant::holds_once(slot, badge) {
            return Err(Error::NotFound);
        }
        crate::handle::grant::spend_once(badge, &termcore::once::digest_call(id, &call))?;
    }
    stop_named(super::identity(slot).name.as_str(), name)
}

/// A process name from ring 3, `len` bytes at `ptr`.
fn name_arg(slot: usize, ptr: u64, len: u64, buf: &mut [u8; MAX_NAME]) -> Result<&str, Error> {
    let len = usize::try_from(len).map_err(|_| Error::Invalid)?;
    if len == 0 || len > MAX_NAME {
        return Err(Error::TooBig);
    }
    copy_from_user(slot, ptr, &mut buf[..len])?;
    core::str::from_utf8(&buf[..len]).map_err(|_| Error::Invalid)
}

/// [`proc_stop`] for the process named `caller`.
pub fn stop_named(caller: &str, name: &str) -> Result<u64, Error> {
    if crate::sched_control::PROTECTED.contains(&name) || name == caller {
        return Err(Error::Permission);
    }
    let found = super::named(name);
    if found.is_empty() {
        return Err(Error::NotFound);
    }
    if found.iter().any(|&(_, supervised)| supervised) {
        return Err(Error::Invalid);
    }
    let stopped = found
        .iter()
        .filter(|&&(pid, _)| super::kill(pid, ExitReason::Killed).is_ok())
        .count() as u64;
    log!("process: {name} stopped ({stopped} of {})", found.len());
    Ok(stopped)
}

/// The `kill` system call: only services, never the supervisor itself.
pub fn kill_service(pid: u64) -> Result<u64, Error> {
    if pid == supervisor_pid() {
        return Err(Error::Permission);
    }
    super::kill(pid, ExitReason::Killed).map(|()| 0)
}

pub fn state_save(slot: usize, ptr: u64, len: u64) -> Result<u64, Error> {
    let len = usize::try_from(len).map_err(|_| Error::TooBig)?;
    if len > MAX_STATE {
        return Err(Error::TooBig);
    }
    let mut staged = Vec::new();
    staged.try_reserve_exact(len).map_err(|_| Error::NoMemory)?;
    staged.resize(len, 0);
    // Copied in full before anything is replaced, so a bad pointer changes nothing.
    copy_from_user(slot, ptr, &mut staged)?;
    threads::preempt_disable();
    let old = {
        let mut state = STATE.lock();
        state.saves += 1;
        core::mem::replace(&mut state.bytes, staged)
    };
    threads::preempt_enable();
    drop(old);
    Ok(len as u64)
}

pub fn state_load(slot: usize, ptr: u64, cap: u64) -> Result<u64, Error> {
    threads::preempt_disable();
    let copy = {
        let state = STATE.lock();
        let mut copy = Vec::new();
        let ok = copy.try_reserve_exact(state.bytes.len()).is_ok();
        if ok {
            copy.extend_from_slice(&state.bytes);
        }
        ok.then_some(copy)
    };
    threads::preempt_enable();
    let copy = copy.ok_or(Error::NoMemory)?;
    let len = copy.len().min(usize::try_from(cap).unwrap_or(usize::MAX));
    copy_to_user(slot, ptr, &copy[..len])?;
    Ok(copy.len() as u64)
}

fn clear_state() {
    threads::preempt_disable();
    let old = core::mem::take(&mut STATE.lock().bytes);
    threads::preempt_enable();
    drop(old);
}

/// The routes (`uses=`), folders (`dir=`) and boot files (`boot=`) the kernel's
/// plan grants service `name` running `program` (`supervise::Plan::grants`);
/// none for a service it does not list, or whose line does not parse.
pub fn plan_grants(name: &str, program: &str) -> Result<supervise::Grants, String> {
    match PLAN.get() {
        Some(plan) => supervise::parse_plan(plan).grants(name, program),
        None => Ok(Default::default()),
    }
}

pub fn plan(slot: usize, ptr: u64, cap: u64) -> Result<u64, Error> {
    let plan = PLAN.get().map_or("", String::as_str).as_bytes();
    let len = plan.len().min(usize::try_from(cap).unwrap_or(usize::MAX));
    copy_to_user(slot, ptr, &plan[..len])?;
    Ok(plan.len() as u64)
}

pub fn list(slot: usize, ptr: u64, cap: u64) -> Result<u64, Error> {
    let mut text = String::new();
    super::services(|pid, name| {
        let _ = writeln!(text, "{pid} {}", name.as_str());
    });
    let len = text.len().min(usize::try_from(cap).unwrap_or(usize::MAX));
    copy_to_user(slot, ptr, &text.as_bytes()[..len])?;
    Ok(text.len() as u64)
}

/// Starts supervision: the init thread, which starts the supervisor with `plan`.
pub fn start(plan: String) -> Result<(), &'static str> {
    log!("supervision: plan:");
    for line in plan.lines() {
        log!("  {}", redacted(line));
    }
    PLAN.call_once(|| plan);
    start_unsupervised();
    threads::spawn("init", init_thread, 0)?;
    Ok(())
}

/// Starts the plan's `unsupervised` lines once, as the kernel's own children with
/// no capabilities: nothing restarts them, so `proc_stop` stops them for good.
fn start_unsupervised() {
    let services = PLAN
        .get()
        .map(|plan| supervise::parse_plan(plan).services)
        .unwrap_or_default();
    for service in services.iter().filter(|s| s.unsupervised) {
        let limits = Limits {
            caps: 0,
            ..service.limits
        };
        match spawn(&SpawnRequest {
            name: &service.name,
            program: &service.program,
            args: service.args.as_bytes(),
            limits,
            spawner: 0,
            supervisor: false,
            uses: &[],
        }) {
            Ok(pid) => log!("process {} (pid {pid}) started unsupervised", service.name),
            Err(error) => log!("supervision: cannot start {}: {error:?}", service.name),
        }
    }
}

fn init_thread(_: usize) {
    INIT_TID.store(threads::current_tid() + 1, Ordering::SeqCst);
    let mut early_deaths = 0;
    loop {
        let exits = SUPERVISOR_EXITS.load(Ordering::SeqCst);
        REACHED_LOOP.store(false, Ordering::SeqCst);
        match spawn(&SpawnRequest {
            name: "supervisor",
            program: "supervisor",
            args: b"",
            limits: SUPERVISOR_LIMITS,
            spawner: 0,
            supervisor: true,
            uses: &[],
        }) {
            Ok(pid) => {
                SUPERVISOR_PID.store(pid, Ordering::SeqCst);
                log!("supervision: supervisor started (pid {pid})");
            }
            Err(error) => {
                log!("supervision: cannot start the supervisor: {error:?}");
                threads::sleep_ms(1000);
                continue;
            }
        }
        while SUPERVISOR_EXITS.load(Ordering::SeqCst) == exits {
            let deadline = hpet::nanos() + 1_000_000_000;
            threads::sleep_until_unless(deadline, || {
                SUPERVISOR_EXITS.load(Ordering::SeqCst) != exits
            });
        }
        SUPERVISOR_PID.store(0, Ordering::SeqCst);
        early_deaths = if REACHED_LOOP.load(Ordering::SeqCst) {
            0
        } else {
            early_deaths + 1
        };
        if early_deaths >= EARLY_DEATHS_BEFORE_RESET {
            log!("supervision: supervisor keeps dying at start; clearing its saved state");
            clear_state();
            early_deaths = 0;
        }
        log!("supervision: supervisor died; restarting it");
        threads::sleep_ms(100);
    }
}

/// A plan line with token values hidden, for the log.
fn redacted(line: &str) -> String {
    line.split(' ')
        .map(|word| match word.split_once('=') {
            Some((key, _)) if key == "token" || key.ends_with("_token") => {
                alloc::format!("{key}=...")
            }
            _ => String::from(word),
        })
        .collect::<Vec<_>>()
        .join(" ")
}
