//! What the kernel does for supervision.
//!
//! - The init thread starts the ring-3 supervisor and restarts it whenever it dies.
//! - Exits of the services the supervisor started queue here, not in the supervisor,
//!   so none is lost while it is down. A record handed out by `wait_child` stays at
//!   the head until the next `wait_child` call (which acknowledges it); if the
//!   supervisor dies first, its successor gets the record again.
//! - The supervisor's saved state lives in a kernel buffer that outlives it.

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
    caps: cap::SUPERVISE,
    stack_pages: 0,
};
/// Limits for a service spawned without its own.
const DEFAULT_PAGES: u64 = 1024;
const DEFAULT_BURST_MS: u64 = 2000;
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

pub(super) fn record_load_failure(pid: u64, name: &Name, supervisor: bool) {
    let process = Process {
        pid,
        name: *name,
        supervisor,
        ..Process::FREE
    };
    on_exit(&process, ExitReason::LoadFailed, 0);
}

/// Reports an ended process: to the init thread if it was the supervisor, to the
/// exit queue if the supervisor started it.
pub(super) fn on_exit(process: &Process, reason: ExitReason, detail: u64) {
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
        0
    };
    if wake != 0 {
        x86_64::instructions::interrupts::without_interrupts(|| threads::wake(wake - 1));
    }
}

fn with_queue<R>(f: impl FnOnce(&mut ExitQueue) -> R) -> R {
    threads::preempt_disable();
    let result = f(&mut QUEUE.lock());
    threads::preempt_enable();
    result
}

/// The `wait_child` system call.
pub fn wait_child(slot: usize, out: u64, timeout_ms: u64) -> Result<u64, Error> {
    REACHED_LOOP.store(true, Ordering::SeqCst);
    let deadline = hpet::nanos().saturating_add(timeout_ms.min(MAX_WAIT_MS) * 1_000_000);
    let me = threads::current_tid();
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
        if hpet::nanos() >= deadline {
            return Ok(0);
        }
        WAITER.store(me + 1, Ordering::SeqCst);
        threads::sleep_until_unless(deadline, || {
            QUEUED.load(Ordering::SeqCst) > 0 || super::killed(slot)
        });
        WAITER.store(0, Ordering::SeqCst);
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
    if name == "supervisor" || request.limits.caps & !cap::GRANTABLE != 0 {
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
        stack_pages: request.limits.stack_pages.min(kabi::layout::MAX_STACK_PAGES),
    };
    let me = super::identity(slot).pid;
    let pid = spawn(&SpawnRequest {
        name,
        args: &args[..args_len],
        limits,
        spawner: me,
        supervisor: false,
    })?;
    log!("process {name} (pid {pid}) started by the supervisor");
    Ok(pid)
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
        log!("  {line}");
    }
    PLAN.call_once(|| plan);
    threads::spawn("init", init_thread, 0)?;
    Ok(())
}

fn init_thread(_: usize) {
    INIT_TID.store(threads::current_tid() + 1, Ordering::SeqCst);
    let mut early_deaths = 0;
    loop {
        let exits = SUPERVISOR_EXITS.load(Ordering::SeqCst);
        REACHED_LOOP.store(false, Ordering::SeqCst);
        match spawn(&SpawnRequest {
            name: "supervisor",
            args: b"",
            limits: SUPERVISOR_LIMITS,
            spawner: 0,
            supervisor: true,
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
