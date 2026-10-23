//! Per-process handle tables (ADR 0001 D1, D3), wiring `captable` into the kernel.
//!
//! Each process slot has a table, filled at spawn with startup handles made from
//! its capability bits (`startup`), which now say only which kinds the process may
//! ever hold. A call that needed a bit now needs a live handle of the matching kind
//! and right (`need`), so closing the handle drops the right for good. The table
//! goes when the process's last thread leaves.
//!
//! Tables live here rather than in `Process`, which is `Copy` and copied under the
//! process table's lock. Lock order: a slot's SPACE, then HANDLES, then TABLE; this
//! module never takes TABLE while holding a table.

use crate::process::MAX_PROCESSES;
use alloc::vec::Vec;
use captable::{Entry, Table};
use kabi::cap;
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP, Rights, netdest};
use spin::Mutex;

/// A process's table and the handles it started with.
pub struct Handles {
    pub table: Table,
    startup: Vec<Handle>,
}

static HANDLES: [Mutex<Option<Handles>>; MAX_PROCESSES] =
    [const { Mutex::new(None) }; MAX_PROCESSES];

/// Rights every startup handle carries besides its own: it can be inspected and
/// narrowed into a second handle.
const BASE: Rights = Rights::DUPLICATE.union(Rights::INSPECT);

/// The startup handles for capability bits `caps` (ADR 0001 D3's table), for the
/// process `pid`. CALL gives one route that still calls by name until handle IPC.
pub fn startup(caps: u64, pid: u64) -> Vec<Entry> {
    let rows: [(u64, Kind, Rights, u64); 11] = [
        (
            cap::SUPERVISE,
            Kind::Job,
            Rights::SPAWN.union(Rights::WAIT),
            0,
        ),
        (
            cap::SERVE,
            Kind::Endpoint,
            Rights::RECV.union(Rights::MINT),
            pid,
        ),
        (cap::CALL, Kind::Service, Rights::CALL, 0),
        (cap::NET, Kind::NetDest, Rights::CONNECT, netdest::LLM),
        (
            cap::POLICY,
            Kind::Sched,
            Rights::READ.union(Rights::SET_POLICY),
            0,
        ),
        (
            cap::COMPONENT,
            Kind::Component,
            Rights::READ.union(Rights::WRITE),
            pid,
        ),
        (
            cap::PUBLISH,
            Kind::Store,
            Rights::READ.union(Rights::WRITE),
            0,
        ),
        (cap::PUBLISH, Kind::Build, Rights::CONNECT, 0),
        (cap::MODEL, Kind::Model, Rights::MAP, 0),
        (cap::THREADS, Kind::Process, Rights::THREADS, pid),
        (
            cap::RESOLVE,
            Kind::NetDest,
            Rights::RESOLVE,
            netdest::RESOLVER,
        ),
    ];
    rows.iter()
        .filter(|(bit, ..)| caps & bit != 0)
        .map(|&(_, kind, rights, object)| Entry::new(kind, rights | BASE, object))
        .collect()
}

/// The handle kind and right that stand for capability bit `bit` in `need`;
/// `None` for a bit no handle stands for.
pub fn required(bit: u64) -> Option<(Kind, Rights)> {
    Some(match bit {
        cap::SUPERVISE => (Kind::Job, Rights::SPAWN),
        cap::SERVE => (Kind::Endpoint, Rights::RECV),
        cap::CALL => (Kind::Service, Rights::CALL),
        cap::NET => (Kind::NetDest, Rights::CONNECT),
        cap::POLICY => (Kind::Sched, Rights::SET_POLICY),
        cap::COMPONENT => (Kind::Component, Rights::WRITE),
        cap::PUBLISH => (Kind::Store, Rights::WRITE),
        cap::MODEL => (Kind::Model, Rights::MAP),
        cap::THREADS => (Kind::Process, Rights::THREADS),
        cap::RESOLVE => (Kind::NetDest, Rights::RESOLVE),
        _ => return None,
    })
}

/// Builds the table for a process about to start in `slot`: its startup handles
/// from `caps`. Before the process runs, so nothing races it.
pub fn create(slot: usize, caps: u64, pid: u64) -> Result<(), kabi::Error> {
    let mut table = Table::new().map_err(kabi::Error::from)?;
    let mut startup = Vec::new();
    startup
        .try_reserve_exact(MAX_STARTUP)
        .map_err(|_| kabi::Error::NoMemory)?;
    for entry in self::startup(caps, pid) {
        startup.push(table.insert(entry).map_err(kabi::Error::from)?);
    }
    *HANDLES[slot].lock() = Some(Handles { table, startup });
    Ok(())
}

/// Drops `slot`'s table, once its process is gone.
pub fn destroy(slot: usize) {
    HANDLES[slot].lock().take();
}

/// Runs `f` on `slot`'s table; NotFound if it has none.
pub fn with<R>(slot: usize, f: impl FnOnce(&mut Handles) -> R) -> Result<R, kabi::Error> {
    HANDLES[slot]
        .lock()
        .as_mut()
        .map(f)
        .ok_or(kabi::Error::NotFound)
}

/// Whether `slot` holds a live handle of `kind` with every right in `rights`.
pub fn holds(slot: usize, kind: Kind, rights: Rights) -> bool {
    HANDLES[slot]
        .lock()
        .as_ref()
        .is_some_and(|handles| handles.table.holds(kind, rights))
}

/// What a call that needed capability bits `bits` now needs: for each bit, a live
/// handle of its kind and right. Permission otherwise.
pub fn need(slot: usize, bits: u64) -> Result<(), kabi::Error> {
    let guard = HANDLES[slot].lock();
    let table = &guard.as_ref().ok_or(kabi::Error::Permission)?.table;
    let mut rest = bits;
    while rest != 0 {
        let bit = rest.isolate_lowest_one();
        rest &= !bit;
        let (kind, rights) = required(bit).ok_or(kabi::Error::Permission)?;
        if !table.holds(kind, rights) {
            return Err(kabi::Error::Permission);
        }
    }
    Ok(())
}

/// A handle as ring 3 sees it.
pub fn info(handle: Handle, entry: &Entry) -> HandleInfo {
    HandleInfo {
        handle: handle.0,
        kind: entry.kind as u32,
        rights: entry.rights.bits(),
        flags: entry.flags.bits(),
    }
}

impl Handles {
    /// The startup handles still open, in the order they were made.
    pub fn startup(&self) -> impl Iterator<Item = HandleInfo> + '_ {
        self.startup
            .iter()
            .filter_map(|&handle| self.table.get(handle).ok().map(|entry| info(handle, entry)))
    }
}
