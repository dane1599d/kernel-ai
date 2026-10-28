//! Per-process handle tables (ADR 0001 D1, D3), wiring `captable` into the kernel.
//!
//! Each process slot has a table, filled at spawn with startup handles made from
//! its capability bits (`startup`), which now say only which kinds the process may
//! ever hold. A call that needed a bit now needs a live handle of the matching kind
//! and right (`need`), so closing the handle drops the right for good. The table
//! goes when the process's last thread leaves.
//!
//! A process with CALL gets one `Service` route per service its plan line names in
//! `uses=` (ADR 0001 D3), in that order; a route names its service by an id from
//! [`route_id`]. Handles move between tables only through handle IPC, each with
//! TRANSFER and into a process whose capability bits allow its kind
//! ([`take_for`]).
//!
//! Tables live here rather than in `Process`, which is `Copy` and copied under the
//! process table's lock. Lock order: a slot's SPACE, then HANDLES, then TABLE; this
//! module never takes TABLE while holding a table.

use crate::process::{MAX_PROCESSES, Name};
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

pub mod grant;
pub mod rules;

static HANDLES: [Mutex<Option<Handles>>; MAX_PROCESSES] =
    [const { Mutex::new(None) }; MAX_PROCESSES];

/// Rights every startup handle carries besides its own: it can be inspected,
/// narrowed into a second handle and moved to another process.
const BASE: Rights = Rights::DUPLICATE
    .union(Rights::INSPECT)
    .union(Rights::TRANSFER);

/// Services routes can name: the id of a route's service is its index here, plus 1.
const MAX_ROUTE_NAMES: usize = 32;
static ROUTE_NAMES: Mutex<[Option<Name>; MAX_ROUTE_NAMES]> =
    Mutex::new([const { None }; MAX_ROUTE_NAMES]);

/// The id routes to the service `name` carry as their object, made on first use.
pub fn route_id(name: &str) -> Result<u64, kabi::Error> {
    let name = Name::new(name).ok_or(kabi::Error::Invalid)?;
    let mut names = ROUTE_NAMES.lock();
    let index = match names.iter().position(|n| *n == Some(name)) {
        Some(index) => index,
        None => {
            let free = names
                .iter()
                .position(Option::is_none)
                .ok_or(kabi::Error::NoMemory)?;
            names[free] = Some(name);
            free
        }
    };
    Ok(index as u64 + 1)
}

/// The service a route with object `id` names.
pub fn route_name(id: u64) -> Option<Name> {
    let index = usize::try_from(id).ok()?.checked_sub(1)?;
    *ROUTE_NAMES.lock().get(index)?
}

/// The startup handles for capability bits `caps` (ADR 0001 D3's table), for the
/// process `pid`. CALL gives a route only through `uses=`, so none here.
pub fn startup(caps: u64, pid: u64) -> Vec<Entry> {
    let rows: [(u64, Kind, Rights, u64); 19] = [
        // MINT under the `system` root, which is never revoked (ADR 0001 D3).
        (
            cap::SUPERVISE,
            Kind::Grant,
            Rights::MINT,
            captable::GrantRef {
                node: captable::grant::SYSTEM,
                generation: 1,
            }
            .pack(),
        ),
        (
            cap::SUPERVISE,
            Kind::Job,
            Rights::SPAWN.union(Rights::WAIT),
            0,
        ),
        (cap::STOP, Kind::Job, Rights::STOP, 0),
        // CREATE, not SPAWN: it starts `scriptrun` only, and never passes for SUPERVISE.
        (
            cap::SCRIPT,
            Kind::Job,
            Rights::CREATE.union(Rights::WAIT),
            crate::process::job::SCRIPTRUN,
        ),
        (
            cap::SERVE,
            Kind::Endpoint,
            Rights::RECV.union(Rights::MINT),
            pid,
        ),
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
        // `kai-data` only: no handle names the ESP or the system slots (ADR 0004 §7).
        (
            cap::PARTITION,
            Kind::Partition,
            Rights::READ.union(Rights::WRITE),
            kabi::part::DATA,
        ),
        // The screen and the input queue, `desktop`'s alone (ADR 0005 §2).
        (cap::DISPLAY, Kind::Display, Rights::WRITE, 0),
        (cap::INPUT, Kind::Input, Rights::READ, 0),
        // The console's rows, for the terminal window: read-only.
        (cap::TERM, Kind::Term, Rights::READ, 0),
        // The slots and the boot record, never the ESP or `kai-data` (ADR 0008 §10).
        (
            cap::UPDATE,
            Kind::Update,
            Rights::INSPECT.union(Rights::READ).union(Rights::WRITE),
            kabi::update::SYSTEM,
        ),
        // The asks for the card, `desktop`'s alone (ADR 0005 §7).
        (cap::ASK, Kind::Ask, Rights::READ.union(Rights::WRITE), 0),
    ];
    rows.iter()
        .filter(|(bit, ..)| caps & bit != 0)
        .map(|&(_, kind, rights, object)| Entry::new(kind, rights | BASE, object))
        .collect()
}

/// FILES's startup handle: `/home/user` on the `fs` route `fs`. Its badge names
/// the folder for `fs`, which looks it up itself (ADR 0004 §7).
fn home_dir(fs: u64) -> Entry {
    let mut home = Entry::new(Kind::Dir, HOME_RIGHTS | BASE, fs);
    home.badge = kabi::fs::HOME_USER;
    home
}

/// The startup `Dir`s on the `fs` route `fs` that `caps` give, in STARTUP's
/// order: FILES's `/home/user`, then SYSTEM_STATE's `/system/state` (the order
/// is ABI, `kabi::cap::SYSTEM_STATE`).
pub fn fs_dirs(caps: u64, fs: u64) -> impl Iterator<Item = Entry> {
    let files = (caps & cap::FILES != 0).then(|| home_dir(fs));
    let state = (caps & cap::SYSTEM_STATE != 0).then(|| state_dir(fs));
    files.into_iter().chain(state)
}

/// SYSTEM_STATE's startup handle: `/system/state` on the `fs` route `fs`, which
/// `fs` looks up itself as for the home folder.
fn state_dir(fs: u64) -> Entry {
    let mut state = Entry::new(Kind::Dir, HOME_RIGHTS | BASE, fs);
    state.badge = kabi::fs::SYSTEM_STATE;
    state
}

/// What the startup `Dir` allows: everything a folder's handle can.
const HOME_RIGHTS: Rights = Rights::READ
    .union(Rights::WRITE)
    .union(Rights::ENUMERATE)
    .union(Rights::CREATE)
    .union(Rights::DELETE);

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
        cap::STOP => (Kind::Job, Rights::STOP),
        cap::SCRIPT => (Kind::Job, Rights::CREATE),
        cap::PARTITION => (Kind::Partition, Rights::READ),
        cap::FILES | cap::SYSTEM_STATE => (Kind::Dir, Rights::READ),
        cap::DISPLAY => (Kind::Display, Rights::WRITE),
        cap::INPUT => (Kind::Input, Rights::READ),
        cap::TERM => (Kind::Term, Rights::READ),
        cap::UPDATE => (Kind::Update, Rights::WRITE),
        cap::ASK => (Kind::Ask, Rights::WRITE),
        _ => return None,
    })
}

/// The kinds a process with capability bits `caps` may hold (its kind ceiling,
/// ADR 0001 D3). A kind no bit stands for yet (`Grant`, `Shm`) is not limited.
pub fn ceiling_allows(caps: u64, kind: Kind) -> bool {
    let bits = [
        (Kind::Job, cap::SUPERVISE | cap::STOP | cap::SCRIPT),
        (Kind::Endpoint, cap::SERVE),
        (Kind::Service, cap::CALL),
        (Kind::Sched, cap::POLICY),
        (Kind::Component, cap::COMPONENT),
        (Kind::Store, cap::PUBLISH),
        (Kind::Build, cap::PUBLISH),
        (Kind::Model, cap::MODEL),
        (Kind::Process, cap::THREADS),
        (Kind::NetDest, cap::NET | cap::RESOLVE),
        (Kind::Partition, cap::PARTITION),
        (Kind::Dir, cap::FILES | cap::SYSTEM_STATE),
        (Kind::File, cap::FILES | cap::SYSTEM_STATE),
        (Kind::Display, cap::DISPLAY),
        (Kind::Input, cap::INPUT),
        (Kind::Term, cap::TERM),
        (Kind::Update, cap::UPDATE),
        (Kind::Ask, cap::ASK),
    ];
    bits.iter()
        .find(|(k, _)| *k == kind)
        .is_none_or(|(_, bit)| caps & bit != 0)
}

/// A route to `service` with CALL, for a startup table or a kernel test.
pub fn route(service: &str) -> Result<Entry, kabi::Error> {
    Ok(Entry::new(
        Kind::Service,
        Rights::CALL | BASE,
        route_id(service)?,
    ))
}

/// Builds the table for a process about to start in `slot`: its startup handles
/// from `caps`, then a route to each service in `uses` (CALL only). Before the
/// process runs, so nothing races it.
pub fn create(slot: usize, caps: u64, pid: u64, uses: &[&str]) -> Result<(), kabi::Error> {
    let mut table = Table::new().map_err(kabi::Error::from)?;
    let mut startup = Vec::new();
    startup
        .try_reserve_exact(MAX_STARTUP)
        .map_err(|_| kabi::Error::NoMemory)?;
    let routes = uses.iter().filter(|_| caps & cap::CALL != 0);
    let routes: Result<Vec<Entry>, kabi::Error> = routes.map(|name| route(name)).collect();
    let fs = match caps & (cap::FILES | cap::SYSTEM_STATE) {
        0 => None,
        _ => Some(route_id(crate::process::supervision::FS)?),
    };
    for entry in self::startup(caps, pid)
        .into_iter()
        .chain(fs.into_iter().flat_map(|fs| fs_dirs(caps, fs)))
        .chain(routes?)
    {
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
        .is_some_and(|handles| handles.table.holds(kind, rights, &grant::Kernel))
}

/// The slots whose table holds a live handle of `kind`, with any rights.
pub fn holders(kind: Kind) -> Vec<usize> {
    (0..MAX_PROCESSES)
        .filter(|&slot| holds(slot, kind, Rights::empty()))
        .collect()
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
        if !table.holds(kind, rights, &grant::Kernel) {
            return Err(kabi::Error::Permission);
        }
    }
    Ok(())
}

/// Takes `handles` out of `slot`'s table to move them to a process with
/// capability bits `to_caps`: each must be open (BadHandle), carry TRANSFER and be
/// of a kind the receiver may hold (Permission), and appear once (Invalid). On any
/// refusal nothing is taken.
pub fn take_for(slot: usize, handles: &[Handle], to_caps: u64) -> Result<Vec<Entry>, kabi::Error> {
    let mut guard = HANDLES[slot].lock();
    let table = &mut guard.as_mut().ok_or(kabi::Error::BadHandle)?.table;
    for (i, &handle) in handles.iter().enumerate() {
        let entry = table.get(handle).map_err(kabi::Error::from)?;
        if !entry.rights.contains(Rights::TRANSFER) || !ceiling_allows(to_caps, entry.kind) {
            return Err(kabi::Error::Permission);
        }
        if handles[..i].contains(&handle) {
            return Err(kabi::Error::Invalid);
        }
    }
    handles
        .iter()
        .map(|&handle| table.close(handle).map_err(kabi::Error::from))
        .collect()
}

/// Puts `entries` moved in before the process in `slot` first runs (JOB_SPAWN)
/// into its table and lists them in STARTUP, as the handles it starts with.
/// One that does not fit is dropped.
pub fn give_at_start(slot: usize, entries: Vec<Entry>) {
    let mut guard = HANDLES[slot].lock();
    let Some(handles) = guard.as_mut() else {
        return;
    };
    for entry in entries {
        if let Ok(handle) = handles.table.insert(entry)
            && handles.startup.try_reserve(1).is_ok()
        {
            handles.startup.push(handle);
        }
    }
}

/// Puts moved `entries` into `slot`'s table; their new handles. One that does not
/// fit is dropped, and its handle is 0.
pub fn give(slot: usize, entries: Vec<Entry>) -> Vec<Handle> {
    let mut guard = HANDLES[slot].lock();
    let Some(handles) = guard.as_mut() else {
        return alloc::vec![Handle::NONE; entries.len()];
    };
    entries
        .into_iter()
        .map(|entry| handles.table.insert(entry).unwrap_or(Handle::NONE))
        .collect()
}

/// The capability bits of the live process `pid`; `None` for none (the kernel).
pub fn caps_of(pid: u64) -> Option<u64> {
    crate::process::with_table(|table| {
        table
            .iter()
            .find(|p| p.live() && p.pid == pid && pid != 0)
            .map(|p| p.limits.caps)
    })
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
