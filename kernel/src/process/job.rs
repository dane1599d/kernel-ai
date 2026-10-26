//! Jobs that start one program (ADR 0001 D6): the shell's `Job` handle with
//! CREATE and WAIT names `scriptrun`, and JOB_SPAWN starts it as the shell's own
//! child with no capability bits. Its table holds only the handles the caller
//! moves in, run-tier handles narrowed under `request N`, so revoking that node
//! leaves it nothing. Its exit goes to the parent's JOB_WAIT, never to the
//! supervisor, and it is killed if the parent ends first.
//!
//! The pool (ADR 0001: M3 landed between 30 and 150 ms, so `scriptrun` is
//! pooled): JOB_POOL starts up to [`MAX_POOL`] idle children with no handles.
//! JOB_CLAIM gives one a request's handles and arguments, and it takes them with
//! JOB_TAKE; JOB_DONE reports its result to the parent's JOB_WAIT and empties
//! its table, so no handle outlives the request, before it waits again.

use super::uaccess::{copy_from_user, copy_to_user, read_struct};
use super::{ExitReason, Limits, MAX_ARGS, Process, SpawnRequest, identity, kill, spawn_with};
use crate::handle::{self, grant::Kernel};
use crate::{hpet, threads};
use alloc::vec::Vec;
use kabi::handle::{Handle, Kind, Rights};
use kabi::{Error, ExitRecord, JobSpawn};
use sched::Tid;
use spin::Mutex;

/// A `Job` object: the one program its CREATE starts.
pub const SCRIPTRUN: u64 = 1;
/// The program a [`SCRIPTRUN`] Job starts.
pub const PROGRAM: &str = "scriptrun";
/// Most handles JOB_SPAWN moves in.
pub const MAX_MOVED: usize = 8;
/// Longest one JOB_WAIT blocks.
const MAX_WAIT_MS: u64 = 60_000;
/// The shell's limits, as the script engine runs there today.
const LIMITS: Limits = Limits {
    max_pages: 4096,
    max_burst_ms: 5000,
    caps: 0,
    stack_pages: 512,
};

/// Most idle children one parent keeps.
pub const MAX_POOL: usize = 4;
/// Longest one JOB_TAKE blocks.
const MAX_TAKE_MS: u64 = 60_000;

/// A pooled child, and what it holds.
struct Pooled {
    pid: u64,
    parent: u64,
    state: PoolState,
}

enum PoolState {
    Idle,
    /// Claimed, its arguments not yet taken.
    Claimed(Vec<u8>),
    Running,
}

static POOL: Mutex<Vec<Pooled>> = Mutex::new(Vec::new());
/// Pooled children blocked in JOB_TAKE, as (pid, thread).
static TAKERS: Mutex<Vec<(u64, Tid)>> = Mutex::new(Vec::new());

/// Exit records waiting for their parent's JOB_WAIT, as (parent pid, record).
static EXITS: Mutex<Vec<(u64, ExitRecord)>> = Mutex::new(Vec::new());
/// Parents blocked in JOB_WAIT, as (parent pid, thread).
static WAITERS: Mutex<Vec<(u64, Tid)>> = Mutex::new(Vec::new());

fn locked<T, R>(lock: &Mutex<T>, f: impl FnOnce(&mut T) -> R) -> R {
    threads::preempt_disable();
    let result = f(&mut lock.lock());
    threads::preempt_enable();
    result
}

/// The Job `raw` names in `slot`'s table, if it carries `right` and starts
/// `scriptrun`.
fn job(slot: usize, raw: u64, right: Rights) -> Result<(), Error> {
    let handle = Handle(u32::try_from(raw).map_err(|_| Error::BadHandle)?);
    let entry = handle::with(slot, |h| h.table.check(handle, Kind::Job, right, &Kernel))?
        .map_err(Error::from)?;
    match entry.object {
        SCRIPTRUN => Ok(()),
        _ => Err(Error::Permission),
    }
}

/// A request's argument string and the handles it moves.
struct Request {
    args: [u8; MAX_ARGS],
    len: usize,
    handles: Vec<Handle>,
}

fn read_request(slot: usize, request_ptr: u64) -> Result<Request, Error> {
    let request: JobSpawn = read_struct(slot, request_ptr)?;
    let args_len = usize::try_from(request.args_len).map_err(|_| Error::TooBig)?;
    let count = usize::try_from(request.handles_len).map_err(|_| Error::TooBig)?;
    if args_len > MAX_ARGS || count > MAX_MOVED {
        return Err(Error::TooBig);
    }
    let mut args = [0u8; MAX_ARGS];
    copy_from_user(slot, request.args_ptr, &mut args[..args_len])?;
    let mut raw_handles = [0u8; MAX_MOVED * 4];
    copy_from_user(slot, request.handles_ptr, &mut raw_handles[..count * 4])?;
    let handles: Vec<Handle> = raw_handles[..count * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| Handle(u32::from_le_bytes(*b)))
        .collect();
    Ok(Request {
        args,
        len: args_len,
        handles,
    })
}

/// JOB_SPAWN: starts `scriptrun` with the argument string and the moved handles
/// the request names; its pid.
pub fn spawn(slot: usize, raw: u64, request_ptr: u64) -> Result<u64, Error> {
    job(slot, raw, Rights::CREATE)?;
    let request = read_request(slot, request_ptr)?;
    start(
        slot,
        PROGRAM,
        &request.args[..request.len],
        &request.handles,
    )
}

/// Starts a job child named `name` running `scriptrun` with `args`, moving
/// `handles` out of `slot`'s table into it; its pid.
fn start(slot: usize, name: &str, args: &[u8], handles: &[Handle]) -> Result<u64, Error> {
    let parent = identity(slot);
    // Nothing the parent may not hold itself moves in.
    let moved = handle::take_for(slot, handles, parent.limits.caps)?;
    let request = SpawnRequest {
        name,
        program: PROGRAM,
        args,
        limits: LIMITS,
        spawner: parent.pid,
        supervisor: false,
        uses: &[],
    };
    spawn_with(&request, moved, true).map_err(|(error, moved)| {
        // Refused: the handles go back to the parent.
        handle::give(slot, moved);
        error
    })
}

/// JOB_WAIT: the next exit of one of the caller's job children, written to `out`;
/// 1 if one was, 0 after `timeout_ms`.
pub fn wait(slot: usize, raw: u64, out: u64, timeout_ms: u64) -> Result<u64, Error> {
    job(slot, raw, Rights::WAIT)?;
    let me = identity(slot).pid;
    let tid = threads::current_tid();
    let deadline = hpet::nanos().saturating_add(timeout_ms.min(MAX_WAIT_MS) * 1_000_000);
    loop {
        let record = locked(&EXITS, |exits| {
            let at = exits.iter().position(|(parent, _)| *parent == me)?;
            Some(exits.remove(at).1)
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
        locked(&WAITERS, |waiters| waiters.push((me, tid)));
        threads::sleep_until_unless(deadline, || {
            super::killed(slot) || EXITS.lock().iter().any(|(parent, _)| *parent == me)
        });
        locked(&WAITERS, |waiters| waiters.retain(|&(_, t)| t != tid));
    }
}

/// A job child ended: it leaves the pool, and its record waits for its parent,
/// which is woken.
pub(super) fn on_exit(process: &Process, reason: ExitReason, detail: u64) {
    locked(&POOL, |pool| pool.retain(|p| p.pid != process.pid));
    report(process.spawner, process, reason, detail);
}

/// Queues `child`'s record for `parent`'s JOB_WAIT and wakes it.
fn report(parent: u64, child: &Process, reason: ExitReason, detail: u64) {
    let (name, len) = child.name.bytes();
    let record = ExitRecord {
        pid: child.pid,
        reason: reason as u64,
        detail,
        name_len: len as u64,
        name,
    };
    let alive = super::with_table(|table| table.iter().any(|p| p.live() && p.pid == parent));
    if !alive {
        return;
    }
    locked(&EXITS, |exits| exits.push((parent, record)));
    let woken: Vec<Tid> = locked(&WAITERS, |waiters| {
        waiters
            .iter()
            .filter(|(p, _)| *p == parent)
            .map(|&(_, tid)| tid)
            .collect()
    });
    for tid in woken {
        x86_64::instructions::interrupts::without_interrupts(|| threads::wake(tid));
    }
}

/// The process `pid` ended: its job children are killed and their records dropped.
pub(super) fn on_parent_exit(pid: u64) {
    let children: Vec<u64> = super::with_table(|table| {
        table
            .iter()
            .filter(|p| p.live() && p.job && p.spawner == pid)
            .map(|p| p.pid)
            .collect()
    });
    for child in children {
        let _ = kill(child, ExitReason::Killed);
    }
    locked(&EXITS, |exits| exits.retain(|(parent, _)| *parent != pid));
}

/// JOB_POOL: keeps up to `n` (at most [`MAX_POOL`]) idle `scriptrun` children
/// for the caller, with no handles; how many it has.
pub fn pool(slot: usize, raw: u64, n: u64) -> Result<u64, Error> {
    job(slot, raw, Rights::CREATE)?;
    let parent = identity(slot).pid;
    let want = usize::try_from(n).unwrap_or(MAX_POOL).min(MAX_POOL);
    loop {
        let held = locked(&POOL, |pool| {
            pool.iter().filter(|p| p.parent == parent).count()
        });
        if held >= want {
            return Ok(held as u64);
        }
        // `scriptrun-1` .. `scriptrun-4`: one live process per name.
        let name = (1..=MAX_POOL)
            .map(|k| alloc::format!("{PROGRAM}-{k}"))
            .find(|name| super::named(name).is_empty())
            .ok_or(Error::Busy)?;
        let pid = start(slot, &name, b"mode=pooled", &[])?;
        locked(&POOL, |pool| {
            pool.push(Pooled {
                pid,
                parent,
                state: PoolState::Idle,
            })
        });
    }
}

/// JOB_CLAIM: gives an idle pooled child the request's arguments and handles
/// and wakes it, or starts one with them when none is idle; its pid.
pub fn claim(slot: usize, raw: u64, request_ptr: u64) -> Result<u64, Error> {
    job(slot, raw, Rights::CREATE)?;
    let request = read_request(slot, request_ptr)?;
    let parent = identity(slot);
    let idle = locked(&POOL, |pool| {
        pool.iter()
            .find(|p| p.parent == parent.pid && matches!(p.state, PoolState::Idle))
            .map(|p| p.pid)
    });
    let Some(pid) = idle else {
        return claim_new(slot, parent.pid, &request);
    };
    let mut args = Vec::new();
    args.try_reserve_exact(request.len)
        .map_err(|_| Error::NoMemory)?;
    args.extend_from_slice(&request.args[..request.len]);
    let child = super::with_table(|table| table.iter().position(|p| p.live() && p.pid == pid))
        .ok_or(Error::Dead)?;
    let moved = handle::take_for(slot, &request.handles, parent.limits.caps)?;
    handle::give_at_start(child, moved);
    locked(&POOL, |pool| {
        if let Some(p) = pool.iter_mut().find(|p| p.pid == pid) {
            p.state = PoolState::Claimed(args);
        }
    });
    let woken: Vec<Tid> = locked(&TAKERS, |takers| {
        takers
            .iter()
            .filter(|(p, _)| *p == pid)
            .map(|&(_, tid)| tid)
            .collect()
    });
    for tid in woken {
        x86_64::instructions::interrupts::without_interrupts(|| threads::wake(tid));
    }
    Ok(pid)
}

/// No idle child: a new pooled one, started with the claim's handles and its
/// arguments waiting, while the pool has room; else a one-off `scriptrun`.
fn claim_new(slot: usize, parent: u64, request: &Request) -> Result<u64, Error> {
    let held = locked(&POOL, |pool| {
        pool.iter().filter(|p| p.parent == parent).count()
    });
    let name = (1..=MAX_POOL)
        .map(|k| alloc::format!("{PROGRAM}-{k}"))
        .find(|name| super::named(name).is_empty())
        .filter(|_| held < MAX_POOL);
    let Some(name) = name else {
        return start(
            slot,
            PROGRAM,
            &request.args[..request.len],
            &request.handles,
        );
    };
    let mut args = Vec::new();
    args.try_reserve_exact(request.len)
        .map_err(|_| Error::NoMemory)?;
    args.extend_from_slice(&request.args[..request.len]);
    let pid = start(slot, &name, b"mode=pooled", &request.handles)?;
    locked(&POOL, |pool| {
        pool.push(Pooled {
            pid,
            parent,
            state: PoolState::Claimed(args),
        })
    });
    Ok(pid)
}

/// JOB_KILL: ends the caller's job child `pid` (its time limit, Ctrl-C). A
/// pooled child dies too, never back to idle, so a half-run script cannot reach
/// the next request; JOB_WAIT reports it Killed.
pub fn kill_child(slot: usize, raw: u64, pid: u64) -> Result<u64, Error> {
    job(slot, raw, Rights::CREATE)?;
    let parent = identity(slot).pid;
    let mine = super::with_table(|table| {
        table
            .iter()
            .any(|p| p.live() && p.pid == pid && p.job && p.spawner == parent)
    });
    if !mine {
        return Err(Error::NotFound);
    }
    kill(pid, ExitReason::Killed).map(|()| 0)
}

/// JOB_TAKE, in a pooled child: waits until it is claimed and copies the claim's
/// arguments to `buf`; their length. TimedOut after a while with no claim.
pub fn take(slot: usize, buf: u64, cap: u64) -> Result<u64, Error> {
    let me = identity(slot);
    if !me.job {
        return Err(Error::Permission);
    }
    let tid = threads::current_tid();
    let deadline = hpet::nanos().saturating_add(MAX_TAKE_MS * 1_000_000);
    let claimed = |pool: &Vec<Pooled>| {
        pool.iter()
            .any(|p| p.pid == me.pid && matches!(p.state, PoolState::Claimed(..)))
    };
    loop {
        let args = locked(&POOL, |pool| {
            let p = pool.iter_mut().find(|p| p.pid == me.pid)?;
            match core::mem::replace(&mut p.state, PoolState::Running) {
                PoolState::Claimed(args) => Some(args),
                other => {
                    p.state = other;
                    None
                }
            }
        });
        if let Some(args) = args {
            let n = args.len().min(usize::try_from(cap).unwrap_or(usize::MAX));
            copy_to_user(slot, buf, &args[..n])?;
            return Ok(args.len() as u64);
        }
        // Not in the pool yet is waiting too: the parent registers a child it
        // just started a moment after the start returns.
        if super::killed(slot) {
            return Err(Error::Dead);
        }
        if hpet::nanos() >= deadline {
            return Err(Error::TimedOut);
        }
        locked(&TAKERS, |takers| takers.push((me.pid, tid)));
        threads::sleep_until_unless(deadline, || super::killed(slot) || claimed(&POOL.lock()));
        locked(&TAKERS, |takers| takers.retain(|&(_, t)| t != tid));
    }
}

/// JOB_DONE, in a pooled child: reports `code` to the parent's JOB_WAIT (reason
/// Exited, detail `code`), closes every handle it holds, and goes back to idle.
pub fn done(slot: usize, code: u64) -> Result<u64, Error> {
    let me = identity(slot);
    let was_running = locked(&POOL, |pool| {
        let p = pool.iter_mut().find(|p| p.pid == me.pid)?;
        let running = matches!(p.state, PoolState::Running);
        if running {
            p.state = PoolState::Idle;
        }
        Some(running)
    });
    if was_running != Some(true) {
        return Err(Error::Invalid);
    }
    // A fresh, empty table: no handle of this request reaches the next one.
    handle::destroy(slot);
    handle::create(slot, 0, me.pid, &[])?;
    report(me.spawner, &me, ExitReason::Exited, code);
    Ok(0)
}
