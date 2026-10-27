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

mod buffer;
mod lookup;
#[cfg(test)]
mod tests;

use alloc::vec::Vec;
use fsproto::{
    Cursor, Error, How, Kind, Listing, MAX_DATA, MAX_LIST, MAX_ROWS, Outcome, Reply, Request, Stat,
    Version,
};
use kabi::fs::{HOME, HOME_USER, MODELS, SYSTEM};
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

/// GC nodes per idle step: well under ADR 0004's 5 ms slice.
const GC_BUDGET: u64 = 64;

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
    /// Always `Some` between calls; taken only while an abort remounts.
    fs: Option<Fs<D>>,
    /// The caller whose action is open.
    owner: Option<u64>,
    /// `/home/user`'s object id in `home`: what [`HOME_USER`] names.
    home_user: u64,
    /// The clock at mount: kaifs's times are this plus time since boot, so they
    /// never go back across reboots (there is no wall clock yet).
    base: u64,
    /// Writes inside an action not yet given to kaifs ([`buffer`]).
    pending: Option<buffer::Pending>,
    /// The record last read.
    cache: Option<buffer::Cached>,
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
            fs: Some(fs),
            owner: None,
            home_user: 0,
            base,
            pending: None,
            cache: None,
        };
        server.home_user = server.folders()?;
        Ok((server, started))
    }

    fn fs(&mut self) -> &mut Fs<D> {
        self.fs.as_mut().expect("fs is mounted between calls")
    }

    /// Makes the missing [`FOLDERS`] in one commit; `/home/user`'s id.
    fn folders(&mut self) -> Result<u64, kaifs::Error> {
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
        if made {
            fs.commit()?;
        }
        fs.set_volume(HOME.into())?;
        Ok(fs.lookup(ROOT, "user")?.map_or(0, |e| e.ino))
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
        Ok(kabi::fs::badge(SYSTEM, true, ino))
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

    /// Between calls: a slice of the GC when space is low, never inside an action.
    pub fn idle(&mut self) {
        if self.owner.is_some() {
            return;
        }
        let fs = self.fs();
        if !fs.writable() || !(fs.gc_running() || fs.space_low()) {
            return;
        }
        let stepped = (|| {
            if !fs.gc_running() {
                fs.gc_start()?;
            }
            if fs.gc_step(GC_BUDGET)?.done {
                fs.commit()?;
            }
            Ok::<(), kaifs::Error>(())
        })();
        if stepped.is_err() {
            self.drop_changes();
        }
    }

    fn answer(&mut self, call: &Call) -> Result<(Reply, Option<Mint>), Error> {
        let request = fsproto::decode_request(call.request)?;
        let changes = changes(&request);
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
                let committed = if self.fs().is_clean() {
                    Ok(self.fs().committed)
                } else {
                    self.fs().commit()
                };
                match committed {
                    Ok(_) => Ok((reply, mint)),
                    Err(e) => {
                        self.drop_changes();
                        Err(error(e))
                    }
                }
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
        let (volume, file, object) = kabi::fs::parse(badge);
        if !matches!(volume, SYSTEM | HOME | MODELS) {
            return Err(Error::NotFound);
        }
        let object = if badge == HOME_USER {
            self.home_user
        } else {
            object
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
                let generation = self.fs().commit();
                self.owner = None;
                match generation {
                    Ok(generation) => Reply::Committed { generation },
                    Err(e) => {
                        self.drop_changes();
                        return Err(error(e));
                    }
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
                let generation = self.fs().commit().map_err(error)?;
                Reply::Committed { generation }
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
            let rows = fs.restore(txn)?;
            let generation = fs.commit()?;
            Ok::<_, kaifs::Error>((rows, generation))
        })();
        let fs = self.fs();
        let _ = fs.set_volume(volume);
        match done {
            Ok((rows, generation)) => Ok(Reply::Restored {
                rows: rows
                    .into_iter()
                    .take(MAX_ROWS as usize)
                    .map(|(ino, o)| (ino, outcome(o)))
                    .collect(),
                generation,
            }),
            Err(e) => {
                self.drop_changes();
                Err(error(e))
            }
        }
    }

    /// Drops everything since the last commit, an open action with it.
    fn drop_changes(&mut self) {
        self.owner = None;
        self.pending = None;
        self.cache = None;
        let fs = self.fs.take().expect("fs is mounted between calls");
        let volume = fs.volume();
        // A remount that fails leaves no filesystem to serve: `fs` exits and its
        // supervisor starts it again, which mounts the last commit.
        let mut fs = fs.abort().expect("the last commit mounts again");
        let _ = fs.set_volume(volume);
        self.fs = Some(fs);
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
        | Request::Sync => true,
        Request::Stat
        | Request::List { .. }
        | Request::Read { .. }
        | Request::History
        | Request::Changes { .. }
        | Request::Df => false,
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
