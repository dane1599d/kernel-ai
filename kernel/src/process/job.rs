//! Jobs that start one program (ADR 0001 D6): the shell's `Job` handle with
//! CREATE and WAIT names `scriptrun`, and JOB_SPAWN starts it as the shell's own
//! child with no capability bits. Its table holds only the handles the caller
//! moves in, run-tier handles narrowed under `request N`, so revoking that node
//! leaves it nothing. Its exit goes to the parent's JOB_WAIT, never to the
//! supervisor, and it is killed if the parent ends first.

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

/// JOB_SPAWN: starts `scriptrun` with the argument string and the moved handles
/// the request names; its pid.
pub fn spawn(slot: usize, raw: u64, request_ptr: u64) -> Result<u64, Error> {
    job(slot, raw, Rights::CREATE)?;
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
    let parent = identity(slot);
    // Nothing the parent may not hold itself moves in.
    let moved = handle::take_for(slot, &handles, parent.limits.caps)?;
    let request = SpawnRequest {
        name: PROGRAM,
        program: PROGRAM,
        args: &args[..args_len],
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

/// A job child ended: its record waits for its parent, which is woken.
pub(super) fn on_exit(process: &Process, reason: ExitReason, detail: u64) {
    let (name, len) = process.name.bytes();
    let record = ExitRecord {
        pid: process.pid,
        reason: reason as u64,
        detail,
        name_len: len as u64,
        name,
    };
    let parent = process.spawner;
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
