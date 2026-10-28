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
//!   nothing, since object ids are never reused.
//! - `..` never leaves the handle's folder: lookup is path-beneath, as cap-std's.

#![no_std]

extern crate alloc;

pub mod batch;
mod buffer;
mod lookup;
#[cfg(test)]
mod tests;
mod trash;

use alloc::vec::Vec;
use fsproto::{
    Cursor, Error, How, Kind, Listing, MAX_DATA, MAX_LIST, MAX_ROWS, Outcome, Reply, Request, Stat,
    Version,
};
use kabi::fs::{Badge, HOME, HOME_USER, MODELS, SYSTEM, SYSTEM_PKG, SYSTEM_STATE};
use kabi::handle::Rights;
use kaifs::ns::Listed;
use kaifs::{BlockDev, FileType, Fs, Inode, ROOT};

/// The folders `fs` makes at its first mount if `cargo xtask fs mkfs` did not
/// (P2 plan §4.5), by volume.
const FOLDERS: [(u8, &str); 4] = [
    (HOME, "user"),
    (SYSTEM, "settings"),
    (SYSTEM, "state"),
    (SYSTEM, "pkg"),
];

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
    /// This disk's epoch, in every badge but the fixed ones ([`kabi::fs`]).
    epoch: u16,
    /// The clock at mount: kaifs's times are this plus time since boot, so they
    /// never go back across reboots (there is no wall clock yet).
    base: u64,
    /// Writes inside an action not yet given to kaifs ([`buffer`]).
    pending: Option<buffer::Pending>,
    /// The record last read.
    cache: Option<buffer::Cached>,
    /// Whether a caller (a pid) still runs: a dead owner's action is aborted
    /// ([`Server::set_alive`]).
    alive: fn(u64) -> bool,
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
        let (fs, started) = if blank(&mut dev)? {
            (Fs::mkfs(dev, uuid, "kai-data")?, Started::Formatted)
        } else {
            let fs = Fs::mount(dev)?;
            let generation = fs.committed;
            (fs, Started::Mounted { generation })
        };
        let base = fs.clock;
        let mut server = Server {
            fs,
            owner: None,
            home_user: 0,
            system_state: 0,
            system_pkg: 0,
            epoch: 0,
            base,
            pending: None,
            cache: None,
            alive: |_| true,
        };
        let fixed = server.prepare(&uuid)?;
        (
            server.home_user,
            server.system_state,
            server.system_pkg,
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
    fn prepare(&mut self, uuid: &[u8; 16]) -> Result<(u64, u64, u64, u16), kaifs::Error> {
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
        let system_pkg = fs.lookup(ROOT, "pkg")?.map_or(0, |e| e.ino);
        fs.set_volume(HOME.into())?;
        let home_user = fs.lookup(ROOT, "user")?.map_or(0, |e| e.ino);
        Ok((home_user, system_state, system_pkg, epoch))
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

    /// The commit generation now.
    pub fn generation(&mut self) -> u64 {
        self.fs().committed
    }

    /// Answers one call; never fails, a failure is [`Reply::Failed`].
    pub fn call(&mut self, call: &Call) -> Answer {
        let now = self.base.saturating_add(call.now);
        let fs = self.fs();
        fs.clock = fs.clock.max(now);
        let (reply, mint) = match self.answer(call) {
            Ok(done) => done,
            Err(error) => (Reply::Failed(error), None),
        };
        Answer {
            reply: fsproto::encode_reply(&reply),
            mint,
        }
    }

    /// Between calls, never inside an action: when space is low, retention drops
    /// the expired undo snapshots and up to [`DROP_PER_PASS`] of the oldest
    /// (kaifs's space rule), then the GC frees what they held: a slice of it, or
    /// while space is low the whole pass, since a writer that never pauses would
    /// otherwise outrun it. `now` is the time in ns, as [`Call::now`].
    pub fn idle(&mut self, now: u64) {
        self.reap();
        if self.owner.is_some() {
            return;
        }
        let fs = self.fs();
        if !fs.writable() || !(fs.gc_running() || fs.space_low()) {
            return;
        }
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

    fn answer(&mut self, call: &Call) -> Result<(Reply, Option<Mint>), Error> {
        let request = fsproto::decode_request(call.request)?;
        let changes = changes(&request);
        if changes && self.owner.is_some_and(|o| o != call.caller) {
            self.reap();
        }
        if changes {
            self.cache = None;
        }
        match request {
            Request::Write { .. } => {}
            Request::Abort if self.owner == Some(call.caller) => self.pending = None,
            _ => self.write_out()?,
        }
        let target = self.target(call.badge)?;
        if changes && self.owner.is_some_and(|o| o != call.caller) {
            return Err(Error::Busy);
        }
        let in_action = self.owner.is_some();
        let done = self.dispatch(call, &target, request);
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
                // there is nothing else pending, inside one the disk failed.
                if changes && (!in_action || e == Error::Io) {
                    self.drop_changes();
                }
                Err(e)
            }
        }
    }

    /// The volume and object `badge` names, checked to exist with its kind.
    fn target(&mut self, badge: u64) -> Result<Target, Error> {
        let Badge {
            volume,
            file,
            epoch,
            object,
        } = Badge::parse(badge);
        let fixed = matches!(badge, HOME_USER | SYSTEM_STATE | SYSTEM_PKG);
        let ours = epoch == self.epoch || fixed;
        if !matches!(volume, SYSTEM | HOME | MODELS) || !ours {
            return Err(Error::NotFound);
        }
        let object = match badge {
            HOME_USER => self.home_user,
            SYSTEM_STATE => self.system_state,
            SYSTEM_PKG => self.system_pkg,
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
                need(Rights::DELETE | Rights::CREATE)?;
                let fs = self.fs();
                fs.rename(object, &from, object, &to).map_err(error)?;
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
                self.restore(txn, action)?
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
                let changed = self.fs().changes(since).map_err(error)?;
                let more = changed.len() > MAX_ROWS as usize;
                let rows = changed.iter().take(MAX_ROWS as usize);
                Reply::Changes {
                    rows: rows.map(|(ino, inode)| stat(*ino, inode)).collect(),
                    more,
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
        };
        Ok((reply, None))
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

    fn restore(&mut self, txn: u64, action: u64) -> Result<Reply, Error> {
        let fs = self.fs();
        let volume = fs.volume();
        let done = (|| {
            fs.begin(action)?;
            fs.restore(txn)
        })();
        let done = match done {
            Ok(rows) => self.commit().map(|generation| (rows, generation)),
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

    /// Drops everything since the last commit, an open action with it; the
    /// generation now on the disk. That is the last commit, or one an error
    /// interrupted after it had landed whole ([`Fs::abort`]).
    fn drop_changes(&mut self) -> u64 {
        self.owner = None;
        self.pending = None;
        self.cache = None;
        // A disk that no longer mounts leaves nothing to serve: `fs` exits and
        // its supervisor starts it again.
        self.fs.abort().expect("the last commit reads back")
    }

    /// Commits, or drops the changes if the commit fails; a commit that landed
    /// whole despite the error counts as done (its changes are on the disk).
    fn commit(&mut self) -> Result<u64, Error> {
        let before = self.fs.committed;
        match self.fs.commit() {
            Ok(generation) => Ok(generation),
            Err(e) => match self.drop_changes() {
                landed if landed > before => Ok(landed),
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
        | Request::Sync => true,
        Request::Stat
        | Request::List { .. }
        | Request::Read { .. }
        | Request::History
        | Request::Changes { .. }
        | Request::Df
        | Request::Generation
        | Request::TrashList { .. } => false,
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
