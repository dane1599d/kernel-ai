//! The `fs` service's logic (ADR 0004 §6-7, P2 task 8). Each call arrives on a
//! `Dir` or `File` handle `fs` minted: the kernel has checked the handle and
//! passes on its badge, `(volume, object id)` ([`kabi::fs`]), and its rights,
//! and [`Server::call`] answers the `kai-fs 1` request in it from kaifs.
//!
//! - A change outside an action is committed before it is answered, so "done"
//!   means durable (ADR 0004 §3). Inside an action ([`Request::Begin`]) changes
//!   wait for [`Request::Commit`], one commit with its undo record.
//! - One action at a time: while one caller's is open, another's change is Busy.
//! - A badge names an object on disk, not server state, so a restarted `fs`
//!   serves every badge the last one minted; a badge whose object is gone finds
//!   nothing, since object ids are never reused: not after a remove, a restore,
//!   an aborted action or a restart mid-action (`kaifs::fs::OID_WINDOW`).
//! - `..` never leaves the handle's folder: lookup is path-beneath, as cap-std's.

#![no_std]

extern crate alloc;

#[cfg(test)]
mod agentvol_tests;
pub mod batch;
mod buffer;
mod bulk;
#[cfg(test)]
mod bulk_tests;
mod changes;
#[cfg(test)]
mod changes_tests;
#[cfg(test)]
mod clock_tests;
#[cfg(test)]
mod dropped_tests;
#[cfg(test)]
mod durable_tests;
mod lookup;
#[cfg(test)]
mod model_tests;
pub mod models;
#[cfg(test)]
mod oid_tests;
#[cfg(test)]
mod rename_tests;
#[cfg(test)]
mod reserve_tests;
#[cfg(test)]
mod sealed_remove_tests;
#[cfg(test)]
mod sealed_shm_tests;
#[cfg(test)]
mod tests;
mod trash;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use fsproto::{
    Cursor, DropCause, Error, How, Kind, Listing, MAX_DATA, MAX_LIST, MAX_ROWS, MAX_SHM_DATA,
    Outcome, Reply, Request, Stat, Version,
};
use kabi::fs::{
    Badge, HOME, HOME_USER, MODELS, MODELS_ROOT, SYSTEM, SYSTEM_APPS, SYSTEM_PKG, SYSTEM_RESTART,
    SYSTEM_SETTINGS, SYSTEM_STATE, SYSTEM_TRUST, SYSTEM_UPDATES,
};
use kabi::handle::Rights;
use kaifs::allocator::Spend;
use kaifs::ns::Listed;
use kaifs::{BlockDev, FileType, Fs, Inode, ROOT};

/// The folders `fs` makes at its first mount if `cargo xtask fs mkfs` did not
/// (P2 plan §4.5), by volume.
const FOLDERS: [(u8, &str); 7] = [
    (HOME, "user"),
    (SYSTEM, "settings"),
    (SYSTEM, "state"),
    (SYSTEM, "pkg"),
    (SYSTEM, "trust"),
    (SYSTEM, "updates"),
    (SYSTEM, "apps"),
];

/// `/system/state`'s folder for the restart window, which `dir=restart` grants
/// alone ([`SYSTEM_RESTART`]); made at mount, as [`FOLDERS`] are.
const RESTART: &str = "restart";

/// Whether `badge` is a fixed folder read only through it: `/system/settings`
/// and `/system/state/restart`.
fn read_only(badge: u64) -> bool {
    matches!(badge, SYSTEM_SETTINGS | SYSTEM_RESTART)
}

/// How `fs` learns a caller's process name ([`Server::set_name_of`]): it writes
/// the name into the buffer and gives its length, or `None` for a pid that is
/// not running.
pub type NameOf = fn(u64, &mut [u8; kabi::MAX_NAME]) -> Option<usize>;

/// How `fs` learns a live caller's plan capability bits ([`Server::set_caps_of`],
/// PART_PEER_CAPS), or `None` when the call fails: a pid not running, a kernel
/// without the call, or any other error.
pub type CapsOf = fn(u64) -> Option<u64>;

/// The callers whose answer [`Server::is_person`] keeps: pids are never reused,
/// so an answer stays true for the process's life, and a few cover the shell
/// and the apps writing now.
const PEOPLE_KEPT: usize = 16;

/// The most a read-only folder's handle may use, however it was made.
const SETTINGS_RIGHTS: Rights = Rights::READ.union(Rights::ENUMERATE).union(BASE);

/// Rights every minted handle carries, as the kernel's startup handles do.
pub const BASE: Rights = Rights::DUPLICATE
    .union(Rights::TRANSFER)
    .union(Rights::INSPECT);
/// Rights a `Dir` handle can carry, and a `File` handle.
pub const DIR_RIGHTS: Rights = Rights::DUPLICATE
    .union(Rights::TRANSFER)
    .union(Rights::INSPECT)
    .union(Rights::READ)
    .union(Rights::WRITE)
    .union(Rights::ENUMERATE)
    .union(Rights::CREATE)
    .union(Rights::DELETE);
pub const FILE_RIGHTS: Rights = Rights::DUPLICATE
    .union(Rights::TRANSFER)
    .union(Rights::INSPECT)
    .union(Rights::READ)
    .union(Rights::WRITE);

/// The extended attribute on `system`'s root that holds the disk's epoch.
const EPOCH: &str = "kai.fs.epoch";

/// GC nodes per idle step: well under ADR 0004's 5 ms slice.
const GC_BUDGET: u64 = 64;
/// The oldest undo snapshots retention drops before one GC pass while space is
/// low: one action makes one, so one a pass falls behind a busy writer.
const DROP_PER_PASS: usize = 8;

/// One request as the kernel delivered it.
pub struct Call<'a> {
    pub caller: u64,
    pub badge: u64,
    pub rights: Rights,
    pub request: &'a [u8],
    /// Nanoseconds since boot, for the clock.
    pub now: u64,
}

/// A handle `fs` mints on its endpoint and moves with the reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mint {
    pub file: bool,
    pub rights: Rights,
    pub badge: u64,
}

/// The encoded reply, and the handle to move with it.
pub struct Answer {
    pub reply: Vec<u8>,
    pub mint: Option<Mint>,
}

/// How the filesystem came up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Started {
    Mounted {
        generation: u64,
    },
    /// The disk was blank, so `fs` formatted it.
    Formatted,
}

/// `/system/state`'s files an update restore keeps ([`Server::kept`]): the
/// standing rules (`shellcore::rules`).
const KEPT_STATE: &[&str] = &["rules"];
/// Names listed at a time while keeping `/system/trust`.
const KEPT_PAGE: usize = 64;

/// A kept file: its folder, its name, and its bytes (`None`: not there).
type Kept = (u64, String, Option<Vec<u8>>);

pub struct Server<D: BlockDev> {
    fs: Fs<D>,
    /// The caller whose action is open.
    owner: Option<u64>,
    /// `/home/user`'s object id in `home`: what [`HOME_USER`] names.
    home_user: u64,
    /// `/system/state`'s object id in `system`: what [`SYSTEM_STATE`] names.
    system_state: u64,
    /// `/system/pkg`'s object id in `system`: what [`SYSTEM_PKG`] names, the
    /// folder a plan line's `dir=pkg` grants (ADR 0011 §12).
    system_pkg: u64,
    /// `/system/trust` and `/system/updates`: what [`SYSTEM_TRUST`] and
    /// [`SYSTEM_UPDATES`] name, the folders `updated`'s `dir=` grants.
    system_trust: u64,
    system_updates: u64,
    /// `/system/settings`: what [`SYSTEM_SETTINGS`] names, read only.
    system_settings: u64,
    /// `/system/state/restart`: what [`SYSTEM_RESTART`] names, read only.
    system_restart: u64,
    /// `/system/apps`'s object id in `system`: what [`SYSTEM_APPS`] names, the
    /// folder a plan line's `dir=apps` grants.
    system_apps: u64,
    /// This disk's epoch, in every badge but the fixed ones ([`kabi::fs`]).
    epoch: u16,
    /// What kaifs's clock counts from: its times are this plus time since
    /// boot. The clock at mount, or the wall time at boot once
    /// [`Server::set_wall_clock`] gives one past it, so retention counts the
    /// time the machine was off and the clock never goes back across reboots.
    base: u64,
    /// Writes inside an action not yet given to kaifs ([`buffer`]).
    pending: Option<buffer::Pending>,
    /// The callers whose action was dropped without their asking: each one's
    /// changes are refused ([`Error::Dropped`]) until it sends Abort or Begin
    /// itself. A commit or a drop of anyone else's leaves the marks alone.
    dropped: BTreeMap<u64, DropCause>,
    /// The caller of the request being answered: a drop on its own request
    /// is one it hears.
    asking: u64,
    /// The least of the open action's changes' claims on kaifs's reserve: what
    /// its commit may spend ([`Server::spends_reserve`]).
    action_spend: Spend,
    /// The record last read.
    cache: Option<buffer::Cached>,
    /// Whether a caller (a pid) still runs: a dead owner's action is aborted
    /// ([`Server::set_alive`]).
    alive: fn(u64) -> bool,
    /// A caller's process name, for the one folder of `/system/apps` it may
    /// open ([`Server::set_name_of`]).
    name_of: NameOf,
    /// A caller's plan caps, for the person's share of the reserve
    /// ([`Server::set_caps_of`]).
    caps_of: CapsOf,
    /// Callers already asked whether they are the person, oldest first
    /// ([`Server::is_person`]).
    people: Vec<(u64, bool)>,
    /// The model store's blob being written in the open action ([`models`]).
    sealing: Option<models::Sealing>,
    /// What a Rename moved with it, as `fs` read it (ENDPOINT_BADGE): a `Dir`'s
    /// badge and rights, or why the handle it moved cannot be one, for the one
    /// call ([`Server::call_into`]).
    into: Option<Result<(u64, Rights), Error>>,
}

/// The object a checked badge names.
struct Target {
    file: bool,
    object: u64,
    inode: Inode,
}

impl<D: BlockDev> Server<D> {
    /// Mounts `dev`, or formats it when no superblock slot was ever written, then
    /// makes any missing [`FOLDERS`].
    pub fn start(mut dev: D, uuid: [u8; 16]) -> Result<(Self, Started), kaifs::Error> {
        let (mut fs, started) = if blank(&mut dev)? {
            (Fs::mkfs(dev, uuid, "kai-data")?, Started::Formatted)
        } else {
            let fs = Fs::mount(dev)?;
            let generation = fs.committed;
            (fs, Started::Mounted { generation })
        };
        // Past every id the run before could have handed out and never
        // committed (`kaifs::fs::OID_WINDOW`), before a volume is made. Then
        // ADR 0013's `memory` and `index`, on a disk that lacks them; no badge
        // names either (`Server::target`), so no client reaches them here.
        // Until a commit saves the new counters no id goes out; a disk that
        // cannot commit now (full, or failing writes) still starts, and
        // `answer` tries again before the next change. A disk that needed
        // the volumes and cannot save them does not start, as before.
        if fs.writable() {
            fs.skip_unsaved_oids();
            let added = fs.add_agent_volumes()?;
            match fs.commit() {
                Ok(_) => {}
                Err(e) if added > 0 => return Err(e),
                Err(_) => {
                    fs.abort()?;
                }
            }
        }
        let base = fs.clock;
        let mut server = Server {
            fs,
            owner: None,
            home_user: 0,
            system_state: 0,
            system_pkg: 0,
            system_trust: 0,
            system_updates: 0,
            system_settings: 0,
            system_restart: 0,
            system_apps: 0,
            epoch: 0,
            base,
            pending: None,
            dropped: BTreeMap::new(),
            asking: 0,
            action_spend: Spend::All,
            cache: None,
            alive: |_| true,
            name_of: |_, _| None,
            caps_of: |_| None,
            people: Vec::new(),
            sealing: None,
            into: None,
        };
        let fixed = server.prepare(&uuid)?;
        (
            server.home_user,
            server.system_state,
            [
                server.system_pkg,
                server.system_trust,
                server.system_updates,
                server.system_settings,
                server.system_apps,
                server.system_restart,
            ],
            server.epoch,
        ) = fixed;
        Ok((server, started))
    }

    /// How `fs` learns whether a caller still runs. Until set, every caller
    /// counts as alive. With it, an action whose owner has died is aborted, as
    /// if it had sent Abort, once another caller's change would wait on it or
    /// at [`Server::idle`], so one crash between Begin and Commit cannot wedge
    /// every later change.
    pub fn set_alive(&mut self, alive: fn(u64) -> bool) {
        self.alive = alive;
    }

    /// The wall time at boot, ns since 1970 (`CLOCK_NOW` less the time since
    /// boot), when the firmware gave a date. kaifs's clock then runs from it,
    /// so an undo snapshot expires 30 days (`memory`'s one hour) after it was
    /// made however long the machine was off between. A wall time behind the
    /// disk's clock changes nothing: the clock never goes back.
    pub fn set_wall_clock(&mut self, boot_epoch_ns: u64) {
        self.base = self.base.max(boot_epoch_ns);
    }

    /// How `fs` learns a caller's process name. Through [`SYSTEM_APPS`] a
    /// caller may only open its own folder, the one named exactly as it is
    /// ([`Server::own_folder_only`]); until this is set, no name is known and
    /// the folder opens nothing.
    pub fn set_name_of(&mut self, name_of: NameOf) {
        self.name_of = name_of;
    }

    /// How `fs` learns a caller's plan capability bits (PART_PEER_CAPS). The
    /// holder of `cap::POLICY`, which the kernel gives the shell's line alone,
    /// is the person, and their trash and truncate may go a quarter further
    /// into the reserve than any app's ([`Server::spends_reserve`]). Until
    /// this is set, no caller is the person.
    pub fn set_caps_of(&mut self, caps_of: CapsOf) {
        self.caps_of = caps_of;
        self.people.clear();
    }

    /// Whether `caller` is the person: it holds `cap::POLICY`. Asked of the
    /// kernel once a caller, its answer kept for the [`PEOPLE_KEPT`] latest;
    /// the kernel (pid 0) is not. A call that fails is not kept: that request
    /// gets an app's tier and the next one asks again, so one passing failure
    /// never makes the shell an app on the full disk that needs its quarter.
    fn is_person(&mut self, caller: u64) -> bool {
        if caller == 0 {
            return false;
        }
        if let Some(&(_, person)) = self.people.iter().find(|(pid, _)| *pid == caller) {
            return person;
        }
        let Some(caps) = (self.caps_of)(caller) else {
            return false;
        };
        let person = caps & kabi::cap::POLICY != 0;
        if self.people.len() == PEOPLE_KEPT {
            self.people.remove(0);
        }
        self.people.push((caller, person));
        person
    }

    /// `/system/apps` is every app's saved state, one folder an app, and the
    /// plan line's `dir=apps` hands the root (its badge is fixed): through it
    /// the one request allowed is an open of the caller's own folder, by its
    /// exact process name. A sibling, `..`, a longer path, a listing or a
    /// rename is refused, so the handle that open mints, beneath which `..`
    /// stops, is all an app ever reaches.
    fn own_folder_only(&self, caller: u64, request: &Request) -> Result<(), Error> {
        let Request::Open { path, .. } = request else {
            return Err(Error::Permission);
        };
        let mut name = [0; kabi::MAX_NAME];
        let len = (self.name_of)(caller, &mut name).ok_or(Error::Permission)?;
        match core::str::from_utf8(name.get(..len).ok_or(Error::Permission)?) {
            Ok(name) if path == name && kaifs::items::valid_name(name) => Ok(()),
            _ => Err(Error::Permission),
        }
    }

    /// Aborts the open action if its owner is dead; the kernel (0) never dies.
    fn reap(&mut self) {
        if let Some(owner) = self.owner
            && owner != 0
            && !(self.alive)(owner)
        {
            self.drop_changes();
        }
    }

    fn fs(&mut self) -> &mut Fs<D> {
        &mut self.fs
    }

    /// Makes the missing [`FOLDERS`], and the disk's epoch if it has none, in
    /// one commit; `/home/user`'s, `/system/state`'s and `/system/pkg`'s ids and
    /// the epoch.
    fn prepare(&mut self, uuid: &[u8; 16]) -> Result<(u64, u64, [u64; 6], u16), kaifs::Error> {
        let fs = self.fs();
        let writable = fs.writable();
        let mut made = false;
        for (volume, name) in FOLDERS {
            fs.set_volume(volume.into())?;
            if fs.lookup(ROOT, name)?.is_none() && writable {
                fs.mkdir(ROOT, name)?;
                made = true;
            }
        }
        fs.set_volume(SYSTEM.into())?;
        if let Some(state) = fs.lookup(ROOT, "state")?
            && fs.lookup(state.ino, RESTART)?.is_none()
            && writable
        {
            fs.mkdir(state.ino, RESTART)?;
            made = true;
        }
        let epoch = match fs.get_xattr(ROOT, EPOCH)? {
            Some(bytes) if bytes.len() == 2 => u16::from_le_bytes([bytes[0], bytes[1]]),
            _ => {
                // Never 0, which only the fixed badges carry.
                let drawn = u16::from_le_bytes([uuid[0], uuid[1]]) & kabi::fs::EPOCH_MASK;
                let epoch = drawn.max(1);
                if writable {
                    fs.set_xattr(ROOT, EPOCH, &epoch.to_le_bytes())?;
                    made = true;
                }
                epoch
            }
        };
        if made {
            fs.commit()?;
        }
        let system_state = fs.lookup(ROOT, "state")?.map_or(0, |e| e.ino);
        // The plan-line folders: `/system/pkg`, `/system/trust`, `/system/updates`,
        // `/system/settings`, `/system/apps`, and `/system/state/restart`.
        let mut granted = [0; 6];
        let names = ["pkg", "trust", "updates", "settings", "apps"];
        for (id, name) in granted.iter_mut().zip(names) {
            *id = fs.lookup(ROOT, name)?.map_or(0, |e| e.ino);
        }
        if system_state != 0 {
            granted[5] = fs.lookup(system_state, RESTART)?.map_or(0, |e| e.ino);
        }
        fs.set_volume(HOME.into())?;
        let home_user = fs.lookup(ROOT, "user")?.map_or(0, |e| e.ino);
        Ok((home_user, system_state, granted, epoch))
    }

    /// The badge for `object` of `volume` on this disk.
    pub(crate) fn badge(&self, volume: u8, file: bool, object: u64) -> Result<u64, Error> {
        if object > kabi::fs::OBJECT_MAX {
            return Err(Error::NoSpace);
        }
        let epoch = self.epoch;
        Ok(Badge {
            volume,
            file,
            epoch,
            object,
        }
        .raw())
    }

    /// The badge of `name` in `system`'s root, a file made if missing: the
    /// kernel's own S04-2 file, which no handle of a client reaches.
    pub fn system_file(&mut self, name: &str) -> Result<u64, Error> {
        let fs = self.fs();
        fs.set_volume(SYSTEM.into()).map_err(error)?;
        let ino = match fs.lookup(ROOT, name).map_err(error)? {
            Some(entry) => entry.ino,
            None => {
                let ino = fs.create(ROOT, name).map_err(error)?;
                fs.commit().map_err(error)?;
                ino
            }
        };
        self.badge(SYSTEM, true, ino)
    }

    /// The model store's file for `sha256` (`<hex>.gguf` in the `models`
    /// volume's root, sealed) and its `kai-data` block runs in file order, for
    /// MODEL_FILL (P2 task 10); `None` if the store has no such sealed file.
    pub fn model_extents(&mut self, sha256: &[u8; 32]) -> Result<Option<Vec<(u64, u64)>>, Error> {
        let mut name = alloc::string::String::with_capacity(69);
        for byte in sha256 {
            name.push_str(&alloc::format!("{byte:02x}"));
        }
        name.push_str(".gguf");
        let fs = self.fs();
        fs.set_volume(MODELS.into()).map_err(error)?;
        let Some(entry) = fs.lookup(ROOT, &name).map_err(error)? else {
            return Ok(None);
        };
        if !fs.is_sealed(entry.ino).map_err(error)? {
            return Ok(None);
        }
        fs.sealed_extents(entry.ino).map(Some).map_err(error)
    }

    /// The commit generation now.
    pub fn generation(&mut self) -> u64 {
        self.fs().committed
    }

    /// Answers one call; never fails, a failure is [`Reply::Failed`].
    pub fn call(&mut self, call: &Call) -> Answer {
        self.call_shm(call, None)
    }

    /// [`Server::call`] for a request that moved a handle in with it: `into`
    /// is the badge and rights of a `Dir` of this server's (ENDPOINT_BADGE),
    /// the destination of [`Request::Rename`], or the error the handle gives
    /// when it cannot be one (another's, closed, of another kind, more than
    /// one): such a Rename fails with it, never falling back to its own
    /// folder. Any other request answers as if nothing moved.
    pub fn call_into(&mut self, call: &Call, into: Result<(u64, Rights), Error>) -> Answer {
        self.into = Some(into);
        let answer = self.call(call);
        self.into = None;
        answer
    }

    /// [`Server::call`] with the `Shm` the request moved in, mapped read-write:
    /// what [`Request::ReadShm`] fills and [`Request::WriteShm`] takes from.
    pub fn call_shm(&mut self, call: &Call, shm: Option<&mut [u8]>) -> Answer {
        let now = self.base.saturating_add(call.now);
        let fs = self.fs();
        fs.clock = fs.clock.max(now);
        let (reply, mint) = match self.answer(call, shm) {
            Ok(done) => done,
            Err(error) => (Reply::Failed(error), None),
        };
        Answer {
            reply: fsproto::encode_reply(&reply),
            mint,
        }
    }

    /// Between calls, never inside an action: when space is low or an undo
    /// snapshot has expired (`memory`'s live an hour), retention drops the
    /// expired undo snapshots and, while space is low, up to [`DROP_PER_PASS`]
    /// of the oldest (kaifs's space rule), then the GC frees what they held: a
    /// slice of it, or while space is low the whole pass, since a writer that
    /// never pauses would otherwise outrun it. `now` is the time in ns since
    /// boot, as [`Call::now`]; expiry is on the filesystem's clock.
    pub fn idle(&mut self, now: u64) {
        self.reap();
        if self.owner.is_some() {
            return;
        }
        let now = self.base.saturating_add(now);
        let fs = self.fs();
        // Under the reserve with freed blocks waiting: commits give them back.
        let release = fs.reserve_waits_on_pending();
        if !fs.writable() || !(fs.gc_running() || fs.space_low() || fs.expiry_due(now) || release) {
            return;
        }
        // Retention, the GC and these commits free space: they may take the
        // reserve, on a disk growth has filled.
        let was = fs.io.alloc.spend_reserve(Spend::All);
        self.retain_and_collect(now);
        if release && self.fs.is_clean() {
            let _ = self.commit();
        }
        self.fs.io.alloc.spend_reserve(was);
    }

    /// Retention, then a slice of the GC (see [`Server::idle`]).
    fn retain_and_collect(&mut self, now: u64) {
        let fs = self.fs();
        let stepped = (|| {
            if !fs.gc_running() {
                let mut dropped = fs.expire(now)?.len();
                for _ in 1..DROP_PER_PASS {
                    match fs.expire(now)?.len() {
                        0 => break,
                        n => dropped += n,
                    }
                }
                if dropped > 0 {
                    fs.commit()?;
                }
                fs.gc_start()?;
            }
            let budget = if fs.space_low() { u64::MAX } else { GC_BUDGET };
            fs.gc_step(budget).map(|step| step.done)
        })();
        match stepped {
            Ok(true) => {
                let _ = self.commit();
            }
            Ok(false) => {}
            Err(_) => {
                self.drop_changes();
            }
        }
    }

    fn answer(
        &mut self,
        call: &Call,
        shm: Option<&mut [u8]>,
    ) -> Result<(Reply, Option<Mint>), Error> {
        self.asking = call.caller;
        // `/system/settings` and `/system/state/restart` are read only through
        // their fixed badges, whatever rights the handle carries: nothing they
        // open gets more than READ and ENUMERATE either.
        let narrowed;
        let call = match read_only(call.badge) {
            true => {
                narrowed = Call {
                    rights: call.rights & SETTINGS_RIGHTS,
                    ..*call
                };
                &narrowed
            }
            false => call,
        };
        let request = fsproto::decode_request(call.request)?;
        if call.badge == SYSTEM_APPS {
            self.own_folder_only(call.caller, &request)?;
        }
        let changes = changes(&request);
        if let Some(&cause) = self.dropped.get(&call.caller).filter(|_| changes) {
            match request {
                // It knows now: an Abort is its acknowledgement, a Begin its
                // next action.
                Request::Abort => {
                    self.dropped.remove(&call.caller);
                    return Ok((Reply::Done, None));
                }
                Request::Begin { .. } => {
                    self.dropped.remove(&call.caller);
                }
                // Its Commit is told the action is gone, which it hears: its
                // next standalone change, such as an undo, goes through.
                Request::Commit => {
                    self.dropped.remove(&call.caller);
                    return Err(Error::Dropped(cause));
                }
                _ => return Err(Error::Dropped(cause)),
            }
        }
        if changes && read_only(call.badge) {
            return Err(Error::Permission);
        }
        if changes && self.owner.is_some_and(|o| o != call.caller) {
            self.reap();
        }
        if changes {
            self.cache = None;
        }
        match request {
            Request::Write { .. } | Request::WriteShm { .. } => {}
            Request::Abort if self.owner == Some(call.caller) => self.pending = None,
            _ => self.write_out()?,
        }
        let target = self.target(call.badge)?;
        if changes && self.owner.is_some_and(|o| o != call.caller) {
            return Err(Error::Busy);
        }
        // Counters an abort or the start's skip moved are saved before the
        // next change outside an action, so ids go out again (a commit that
        // fails leaves them refused, and the change then fails as it would).
        if changes && self.owner.is_none() && self.fs.oids_unsaved() && self.fs.is_clean() {
            let _ = self.commit();
        }
        let spend = self.spends_reserve(call.caller, &request, changes);
        let was = self.fs.io.alloc.spend_reserve(spend);
        let answered = self.answer_change(call, &target, request, changes, shm);
        self.fs.io.alloc.spend_reserve(was);
        answered
    }

    /// A model blob's removal gets half of the reserve, as a trash does: its
    /// space also comes back later.
    ///
    /// How far `request` may go into kaifs's reserved blocks, so a disk
    /// growth filled can still be emptied, and no client can drain them: all
    /// of it only for work that frees space at once and cannot be looped to
    /// spend it (an emptied trash, a dropped update snapshot), half for a
    /// trash or a truncate, whose space comes back later, down to a quarter
    /// for the person's ([`Server::is_person`]), so no app's trashing keeps
    /// them from deleting, none for growth. An action's commit, by Commit or
    /// its owner's Sync, gets the least of its changes'. Idle's retention and
    /// GC and `check --repair` take all of it themselves.
    pub(crate) fn spends_reserve(
        &mut self,
        caller: u64,
        request: &Request,
        changes: bool,
    ) -> Spend {
        if matches!(request, Request::Begin { .. }) && self.owner.is_none() {
            self.action_spend = Spend::All;
        }
        let in_its_action = self.owner == Some(caller);
        let spend = match request {
            // What frees space at once, and cannot be looped to drain the
            // reserve: an emptied trash, a dropped update snapshot. (Abort
            // allocates nothing.)
            Request::EmptyTrash | Request::UpdateDrop { .. } | Request::Abort => Spend::All,
            // Space that may come back only later, while its copy-on-write
            // costs blocks now, so a loop of it could drain all of the
            // reserve: half, as a trash. The person has their own quarter
            // below the apps' half, so an app that trashes until refused still
            // leaves them room to delete.
            Request::Trash { .. } | Request::Truncate { .. } if self.is_person(caller) => {
                Spend::Person
            }
            Request::Trash { .. } | Request::Truncate { .. } => Spend::Half,
            // A model blob's removal, outside any action, frees the blob once
            // the GC pass it starts has run, and costs copy-on-write now. Each
            // needs a sealed blob, written above the reserve, and one refused
            // writes nothing: half, as a trash, so the GC and an emptied
            // trash keep the rest.
            Request::SealedRemove { .. } => Spend::Half,
            // The commit of an action, by Commit or the owner's own Sync, at
            // the least tier of its changes; a Sync outside one has nothing to
            // write and needs no reserve.
            Request::Commit => self.action_spend,
            Request::Sync if in_its_action => self.action_spend,
            _ => Spend::Keep,
        };
        let commits = matches!(request, Request::Commit | Request::Abort | Request::Sync);
        if changes && in_its_action && !commits {
            self.action_spend = self.action_spend.min(spend);
        }
        spend
    }

    /// [`Server::answer`]'s change, once the request is known to reach its
    /// target.
    fn answer_change(
        &mut self,
        call: &Call,
        target: &Target,
        request: Request,
        changes: bool,
        shm: Option<&mut [u8]>,
    ) -> Result<(Reply, Option<Mint>), Error> {
        let in_action = self.owner.is_some();
        let done = match request {
            Request::ReadShm { off, len } => self
                .read_shm(call, target, off, len, shm)
                .map(|r| (r, None)),
            Request::WriteShm { off, len } => self
                .write_shm(call, target, off, len, shm)
                .map(|r| (r, None)),
            Request::SealedPushShm { len } => self
                .sealed_push_shm(call, target, len, shm)
                .map(|r| (r, None)),
            request => self.dispatch(call, target, request),
        };
        match done {
            // A change outside an action is durable before it is answered.
            Ok((reply, mint)) if changes && !in_action && self.owner.is_none() => {
                if !self.fs.is_clean() {
                    self.commit()?;
                }
                Ok((reply, mint))
            }
            Ok(done) => Ok(done),
            Err(e) => {
                // A failed change leaves nothing half done: outside an action
                // there is nothing else pending, inside one the disk failed,
                // and its owner (the caller) is told its action is gone.
                if changes && !in_action {
                    self.drop_changes();
                } else if changes && e == Error::Io {
                    self.drop_changes();
                    return Err(Error::Dropped(DropCause::Io));
                }
                Err(e)
            }
        }
    }

    /// The folder a Rename on `source` moves into: the `Dir` whose badge and
    /// rights the request moved in. It needs CREATE, must be a folder this
    /// server serves in the same volume (kaifs renames within one), and is
    /// never a read-only folder or the apps root, whose openings are held to
    /// one folder a caller ([`Server::own_folder_only`]).
    fn rename_into(&mut self, source: u64, badge: u64, rights: Rights) -> Result<u64, Error> {
        if !rights.contains(Rights::CREATE) || read_only(badge) || badge == SYSTEM_APPS {
            return Err(Error::Permission);
        }
        if Badge::parse(badge).volume != Badge::parse(source).volume {
            return Err(Error::Invalid);
        }
        let target = self.target(badge)?;
        if target.file {
            return Err(Error::NotDir);
        }
        // `target` chose the destination's volume, the source's too.
        Ok(target.object)
    }

    /// The volume and object `badge` names, checked to exist with its kind.
    fn target(&mut self, badge: u64) -> Result<Target, Error> {
        let Badge {
            volume,
            file,
            epoch,
            object,
        } = Badge::parse(badge);
        let fixed = matches!(
            badge,
            HOME_USER
                | SYSTEM_STATE
                | SYSTEM_PKG
                | SYSTEM_TRUST
                | SYSTEM_UPDATES
                | SYSTEM_SETTINGS
                | SYSTEM_APPS
                | SYSTEM_RESTART
                | MODELS_ROOT
        );
        let ours = epoch == self.epoch || fixed;
        if !matches!(volume, SYSTEM | HOME | MODELS) || !ours {
            return Err(Error::NotFound);
        }
        let object = match badge {
            HOME_USER => self.home_user,
            SYSTEM_STATE => self.system_state,
            SYSTEM_PKG => self.system_pkg,
            SYSTEM_TRUST => self.system_trust,
            SYSTEM_UPDATES => self.system_updates,
            SYSTEM_SETTINGS => self.system_settings,
            SYSTEM_APPS => self.system_apps,
            SYSTEM_RESTART => self.system_restart,
            MODELS_ROOT => ROOT,
            _ => object,
        };
        let fs = self.fs();
        fs.set_volume(volume.into()).map_err(error)?;
        if object == 0 {
            return Err(Error::NotFound);
        }
        let inode = fs.stat(object).map_err(error)?;
        if (inode.kind == FileType::File) != file {
            return Err(Error::NotFound);
        }
        Ok(Target {
            file,
            object,
            inode,
        })
    }

    fn dispatch(
        &mut self,
        call: &Call,
        target: &Target,
        request: Request,
    ) -> Result<(Reply, Option<Mint>), Error> {
        let need = |rights: Rights| {
            if call.rights.contains(rights) {
                Ok(())
            } else {
                Err(Error::Permission)
            }
        };
        let dir = || {
            if target.file {
                Err(Error::NotDir)
            } else {
                Ok(())
            }
        };
        let file = || {
            if target.file {
                Ok(())
            } else {
                Err(Error::IsDir)
            }
        };
        let object = target.object;
        let reply = match request {
            Request::Open { path, rights, how } => {
                dir()?;
                let rights = Rights::from_bits(rights).ok_or(Error::Invalid)?;
                return self.open(call, object, &path, rights, how);
            }
            Request::Stat => Reply::Stat(stat(object, &target.inode)),
            Request::List { after, max } => {
                dir()?;
                need(Rights::ENUMERATE)?;
                self.list(object, after, max)?
            }
            Request::TrashList { after } => {
                dir()?;
                need(Rights::ENUMERATE)?;
                self.trash_list(object, after)?
            }
            Request::TrashRestore { path } => {
                dir()?;
                need(Rights::CREATE)?;
                self.trash_restore(object, &path)?
            }
            Request::EmptyTrash => {
                dir()?;
                need(Rights::DELETE)?;
                self.empty_trash(object)?
            }
            Request::Read { off, len } => {
                file()?;
                need(Rights::READ)?;
                let len = (len as usize).min(MAX_DATA);
                Reply::Data(self.read(object, off, len, target.inode.size)?)
            }
            Request::Write { off, data } => {
                file()?;
                need(Rights::WRITE)?;
                if data.len() > MAX_DATA {
                    return Err(Error::Invalid);
                }
                self.write(object, off, &data)?;
                Reply::Done
            }
            Request::Truncate { size } => {
                file()?;
                need(Rights::WRITE)?;
                self.fs().truncate(object, size).map_err(error)?;
                Reply::Done
            }
            Request::Rename { from, to } => {
                dir()?;
                // Into the moved `Dir` when there is one: DELETE here, CREATE
                // there. Else in this one, which needs both.
                let into = match self.into.take() {
                    Some(Ok((badge, rights))) => {
                        need(Rights::DELETE)?;
                        self.rename_into(call.badge, badge, rights)?
                    }
                    Some(Err(why)) => return Err(why),
                    None => {
                        need(Rights::DELETE | Rights::CREATE)?;
                        object
                    }
                };
                let fs = self.fs();
                fs.rename(object, &from, into, &to).map_err(error)?;
                Reply::Done
            }
            Request::Trash { name } => {
                dir()?;
                need(Rights::DELETE)?;
                self.fs().trash(object, &name).map_err(error)?;
                Reply::Done
            }
            Request::Begin { action } => {
                need(Rights::WRITE)?;
                if self.owner.is_some() {
                    return Err(Error::Busy);
                }
                self.fs().begin(action).map_err(error)?;
                self.owner = Some(call.caller);
                Reply::Done
            }
            Request::Commit => {
                self.owned(call.caller)?;
                // Refused before the action changes hands, so its owner can
                // still finish or abort the blob.
                if self.sealing.is_some() {
                    return Err(Error::Invalid);
                }
                let generation = self.commit();
                self.owner = None;
                Reply::Committed {
                    generation: generation?,
                }
            }
            Request::Abort => {
                self.owned(call.caller)?;
                self.drop_changes();
                Reply::Done
            }
            Request::Restore { txn, action } => {
                need(Rights::WRITE | Rights::DELETE)?;
                if self.owner.is_some() {
                    return Err(Error::Busy);
                }
                self.restore(object, txn, action)?
            }
            Request::History => {
                need(Rights::READ)?;
                let versions = self.fs().history(object).map_err(error)?;
                let rows = versions.iter().take(MAX_ROWS as usize).map(|v| Version {
                    snapshot: v.snapshot,
                    generation: v.generation,
                    stat: stat(object, &v.inode),
                });
                Reply::History(rows.collect())
            }
            Request::Changes { since } => {
                need(Rights::ENUMERATE)?;
                let (rows, more, _) = self.changed_since(object, since, None)?;
                Reply::Changes { rows, more }
            }
            Request::ChangesSince { since, after } => {
                need(Rights::ENUMERATE)?;
                self.save_fallbacks()?;
                let (rows, more, upto) = self.changed_since(object, since, after)?;
                Reply::ChangesPage {
                    rows,
                    more,
                    upto,
                    fallbacks: self.fs().fallbacks(),
                }
            }
            Request::Sync => {
                let generation = self.commit()?;
                Reply::Committed { generation }
            }
            Request::Generation => {
                self.owned(call.caller)?;
                Reply::Generation {
                    generation: self.fs.io.txn,
                }
            }
            Request::Df => {
                let (free, total) = self.fs().space();
                Reply::Space { free, total }
            }
            Request::UpdateSnapshot => {
                self.updates_only(call)?;
                let fs = self.fs();
                fs.set_volume(SYSTEM.into()).map_err(error)?;
                let id = fs
                    .snapshot_as(kaifs::vol::SnapKind::Update, 0, 0)
                    .map_err(error)?;
                let generation = self.commit()?;
                Reply::Snapshot { id, generation }
            }
            Request::UpdateRestore { id } => {
                self.updates_only(call)?;
                let fs = self.fs();
                fs.set_volume(SYSTEM.into()).map_err(error)?;
                let update = fs
                    .snapshots()
                    .get(&id)
                    .is_some_and(|s| s.kind == kaifs::vol::SnapKind::Update);
                if !update {
                    return Err(Error::Invalid);
                }
                let kept = self.kept().map_err(error)?;
                let restored = (|| {
                    let fs = self.fs();
                    if !fs.restore_snapshot(id)? {
                        return Ok(false);
                    }
                    self.put_back(&kept)?;
                    Ok(true)
                })();
                match restored {
                    Ok(true) => {}
                    Ok(false) => return Err(Error::Invalid),
                    Err(e) => {
                        self.drop_changes();
                        return Err(error(e));
                    }
                }
                let generation = self.commit()?;
                Reply::Committed { generation }
            }
            Request::UpdateDrop { id } => {
                self.updates_only(call)?;
                let fs = self.fs();
                fs.set_volume(SYSTEM.into()).map_err(error)?;
                let update = fs
                    .snapshots()
                    .get(&id)
                    .is_some_and(|s| s.kind == kaifs::vol::SnapKind::Update);
                if !update || !fs.drop_snapshot(id).map_err(error)? {
                    return Err(Error::Invalid);
                }
                let generation = self.commit()?;
                Reply::Committed { generation }
            }
            Request::SealedBegin { name, len } => self.sealed_begin(call, target, &name, len)?,
            Request::SealedPush { data } => self.sealed_push(call, target, &data)?,
            Request::SealedFinish => self.sealed_finish(call, target)?,
            Request::SealedRemove { name } => self.sealed_remove(call, target, &name)?,
            Request::ModelHint { name } => self.model_hint(call, target, name.as_deref())?,
            Request::ModelInfo { name } => self.model_info(call, target, &name)?,
            // `answer` takes these with their `Shm` (`bulk.rs`).
            Request::ReadShm { .. } | Request::WriteShm { .. } | Request::SealedPushShm { .. } => {
                return Err(Error::Invalid);
            }
        };
        Ok((reply, None))
    }

    /// What an update restore keeps from the live tree (ADR 0008 §7, §10):
    /// every file in `/system/trust` and `/system/state`'s [`KEPT_STATE`], with
    /// their bytes, or `None` for a kept name that is not there. Going back,
    /// they would bring back a root a rotation revoked, replayed metadata, and
    /// rules the user removed.
    fn kept(&mut self) -> Result<Vec<Kept>, kaifs::Error> {
        let (trust, state) = (self.system_trust, self.system_state);
        let fs = self.fs();
        let mut kept = Vec::new();
        if trust != 0 {
            let mut after = None;
            loop {
                let page = fs.list(trust, after.as_ref(), KEPT_PAGE)?;
                for listed in &page {
                    if listed.entry.kind == FileType::File {
                        let bytes = fs.read_file(listed.entry.ino)?;
                        kept.push((trust, listed.entry.name.clone(), Some(bytes)));
                    }
                }
                if page.len() < KEPT_PAGE {
                    break;
                }
                after = page.into_iter().last();
            }
        }
        if state != 0 {
            for name in KEPT_STATE {
                let bytes = match fs.lookup(state, name)? {
                    Some(entry) if entry.kind == FileType::File => Some(fs.read_file(entry.ino)?),
                    _ => None,
                };
                kept.push((state, (*name).into(), bytes));
            }
        }
        Ok(kept)
    }

    /// Writes [`Server::kept`]'s files back over the restored tree; a trust
    /// file the restored tree has and the live one did not goes.
    fn put_back(&mut self, kept: &[Kept]) -> Result<(), kaifs::Error> {
        let trust = self.system_trust;
        let fs = self.fs();
        if trust != 0 {
            let mut stale = Vec::new();
            let mut after = None;
            loop {
                let page = fs.list(trust, after.as_ref(), KEPT_PAGE)?;
                for listed in &page {
                    let name = &listed.entry.name;
                    if !kept.iter().any(|(dir, n, _)| *dir == trust && n == name) {
                        stale.push(name.clone());
                    }
                }
                if page.len() < KEPT_PAGE {
                    break;
                }
                after = page.into_iter().last();
            }
            for name in stale {
                fs.remove(trust, &name)?;
            }
        }
        for (dir, name, bytes) in kept {
            let there = fs.lookup(*dir, name)?;
            match (bytes, there) {
                (Some(bytes), Some(entry)) => fs.write_file(entry.ino, bytes)?,
                (Some(bytes), None) => {
                    let ino = fs.create(*dir, name)?;
                    fs.write_file(ino, bytes)?;
                }
                (None, Some(_)) => fs.remove(*dir, name)?,
                (None, None) => {}
            }
        }
        Ok(())
    }

    /// The `update` snapshot's requests come only on `/system/updates`'s `Dir`
    /// with WRITE, and never inside an action: each commits on its own, so in
    /// the caller's own action it would commit that action half done (Invalid),
    /// and another caller's is Busy.
    fn updates_only(&self, call: &Call) -> Result<(), Error> {
        if call.badge != SYSTEM_UPDATES || !call.rights.contains(Rights::WRITE) {
            return Err(Error::Permission);
        }
        match self.owner {
            Some(owner) if owner == call.caller => Err(Error::Invalid),
            Some(_) => Err(Error::Busy),
            None => Ok(()),
        }
    }

    fn owned(&self, caller: u64) -> Result<(), Error> {
        match self.owner {
            Some(owner) if owner == caller => Ok(()),
            Some(_) => Err(Error::Busy),
            None => Err(Error::Invalid),
        }
    }

    fn list(&mut self, dir: u64, after: Option<Cursor>, max: u32) -> Result<Reply, Error> {
        let max = max.clamp(1, MAX_LIST) as usize;
        let after = after.map(|c| Listed {
            hash: c.hash,
            entry: kaifs::items::Entry {
                name: c.name,
                ino: 0,
                kind: FileType::File,
            },
        });
        let mut got = self
            .fs()
            .list(dir, after.as_ref(), max + 1)
            .map_err(error)?;
        let more = got.len() > max;
        got.truncate(max);
        let more = more.then(|| got.last()).flatten().map(|l| Cursor {
            hash: l.hash,
            name: l.entry.name.clone(),
        });
        let names = got.into_iter().map(|l| Listing {
            kind: kind(l.entry.kind),
            object: l.entry.ino,
            name: l.entry.name,
        });
        Ok(Reply::Listed {
            names: names.collect(),
            more,
        })
    }

    /// Restores action `txn` as action `action`, if all it changed, and all the
    /// restore changes, lies in folder `dir` (the handle's), its trashed items
    /// counted by where they came from ([`Fs::restore_within`]): a handle
    /// reverts nothing in a folder it was not given.
    fn restore(&mut self, dir: u64, txn: u64, action: u64) -> Result<Reply, Error> {
        let fs = self.fs();
        let volume = fs.volume();
        let done = (|| {
            fs.begin(action)?;
            fs.restore_within(txn, dir)
        })();
        let done = match done {
            Ok(Some(rows)) => self.commit().map(|generation| (rows, generation)),
            Ok(None) => {
                self.drop_changes();
                Err(Error::Permission)
            }
            Err(e) => {
                self.drop_changes();
                Err(error(e))
            }
        };
        let _ = self.fs.set_volume(volume);
        match done {
            Ok((rows, generation)) => Ok(Reply::Restored {
                rows: rows
                    .into_iter()
                    .take(MAX_ROWS as usize)
                    .map(|(ino, o)| (ino, outcome(o)))
                    .collect(),
                generation,
            }),
            Err(e) => Err(e),
        }
    }

    /// [`Server::drop_changes`] for an action whose pending run failed to
    /// write out (`failed` says why); the error to answer. On another caller's
    /// request the owner hears nothing, so its changes are refused
    /// ([`Error::Dropped`]) until it sends Abort or Begin, and the caller gets
    /// `failed`. On the owner's own request (its Write, its Commit) that answer
    /// is [`Error::Dropped`], not why the run failed, so it knows its action
    /// is gone and cannot write on as if inside it; it is not marked, so its
    /// next standalone change, such as an undo, goes through. Callers that
    /// have died since are forgotten first, so the marks stay few.
    pub(crate) fn drop_unasked(&mut self, failed: Error) -> Error {
        let owner = self.owner;
        self.drop_changes();
        let alive = self.alive;
        self.dropped.retain(|&caller, _| alive(caller));
        let cause = match failed {
            Error::NoSpace => DropCause::NoSpace,
            _ => DropCause::Io,
        };
        match owner {
            Some(owner) if owner == self.asking => Error::Dropped(cause),
            Some(owner) => {
                self.dropped.insert(owner, cause);
                failed
            }
            None => failed,
        }
    }

    /// Drops everything since the last commit, an open action with it; the
    /// generation now on the disk. That is the last commit, or one an error
    /// interrupted after it had landed whole ([`Fs::abort`]).
    fn drop_changes(&mut self) -> u64 {
        self.owner = None;
        self.pending = None;
        self.sealing = None;
        self.cache = None;
        // A disk that no longer mounts leaves nothing to serve: `fs` exits and
        // its supervisor starts it again.
        self.fs.abort().expect("the last commit reads back")
    }

    /// Commits, or drops the changes if the commit fails; a commit that landed
    /// whole despite the error counts as done (its changes are on the disk).
    /// A model blob begun and not finished is never made durable, whatever
    /// commits: Commit, Sync, or a change outside an action (S-io's review).
    fn commit(&mut self) -> Result<u64, Error> {
        if self.sealing.is_some() {
            return Err(Error::Invalid);
        }
        let before = self.fs.committed;
        match self.fs.commit() {
            Ok(generation) => Ok(generation),
            // Landed whole despite the error only if the disk says so after
            // a flush that worked: the slot read back may still sit in the
            // device's volatile cache, and a cache must never vouch for it.
            Err(e) => match self.drop_changes() {
                landed if landed > before && self.fs.io.dev.flush().is_ok() => Ok(landed),
                _ => Err(error(e)),
            },
        }
    }
}

/// Whether a request changes the filesystem.
fn changes(request: &Request) -> bool {
    match request {
        Request::Open { how, .. } => *how != How::Existing,
        Request::Write { .. }
        | Request::Truncate { .. }
        | Request::Rename { .. }
        | Request::Trash { .. }
        | Request::Begin { .. }
        | Request::Commit
        | Request::Abort
        | Request::Restore { .. }
        | Request::TrashRestore { .. }
        | Request::EmptyTrash
        | Request::Sync
        | Request::UpdateSnapshot
        | Request::UpdateRestore { .. }
        | Request::UpdateDrop { .. }
        | Request::SealedBegin { .. }
        | Request::SealedPush { .. }
        | Request::SealedFinish
        | Request::SealedRemove { .. }
        | Request::ModelHint { .. }
        | Request::WriteShm { .. }
        | Request::SealedPushShm { .. } => true,
        Request::Stat
        | Request::List { .. }
        | Request::Read { .. }
        | Request::History
        | Request::Changes { .. }
        | Request::ChangesSince { .. }
        | Request::Df
        | Request::Generation
        | Request::TrashList { .. }
        | Request::ModelInfo { .. }
        | Request::ReadShm { .. } => false,
    }
}

/// Whether no superblock slot at either end of `dev` was ever written: a disk
/// `fs` may format. A disk with anything there is never formatted.
fn blank<D: BlockDev>(dev: &mut D) -> Result<bool, kaifs::Error> {
    let blocks = dev.info().blocks;
    if blocks < 2 * kaifs::SLOTS + 2 {
        return Ok(false);
    }
    let mut block = [0u8; kaifs::BLOCK];
    let slots = (1..=kaifs::SLOTS).chain(blocks - kaifs::SLOTS..blocks);
    for addr in slots {
        dev.read(addr, &mut block)?;
        if block.iter().any(|&b| b != 0) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn kind(kind: FileType) -> Kind {
    match kind {
        FileType::File => Kind::File,
        FileType::Dir => Kind::Dir,
    }
}

fn stat(object: u64, inode: &Inode) -> Stat {
    Stat {
        kind: kind(inode.kind),
        object,
        size: inode.size,
        mtime: inode.mtime,
        crtime: inode.crtime,
        changed: inode.changed,
    }
}

fn outcome(outcome: kaifs::undo::Outcome) -> Outcome {
    use kaifs::undo::Outcome as O;
    match outcome {
        O::Back => Outcome::Back,
        O::Moved { name } => Outcome::Moved { name },
        O::Beside { ino, name } => Outcome::Beside { object: ino, name },
        O::Removed => Outcome::Removed,
        O::Left => Outcome::Left,
    }
}

/// kaifs's error as the caller sees it: the disk's own failures are all Io.
fn error(e: kaifs::Error) -> Error {
    use kaifs::Error as E;
    match e {
        E::NotFound => Error::NotFound,
        E::Exists => Error::Exists,
        E::NotDir => Error::NotDir,
        E::IsDir => Error::IsDir,
        E::NotEmpty => Error::NotEmpty,
        E::Invalid(_) => Error::Invalid,
        E::ReadOnly => Error::ReadOnly,
        E::NoSpace => Error::NoSpace,
        E::Io | E::Corrupt(_) | E::NoValidSuperblock | E::Unsupported(_) => Error::Io,
    }
}
