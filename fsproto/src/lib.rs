//! `kai-fs 1` (ADR 0004 §6): what a client sends on a `Dir` or `File` handle and
//! what the `fs` service answers, as CBOR `[version, body]` in one IPC message of
//! at most [`MAX_MESSAGE`], with at most [`MAX_DATA`] bytes of
//! file data in it. The variant and field numbers are ABI: a new field or
//! request bumps nothing old clients read; a changed meaning is `kai-fs 2`.
//!
//! The handle a request comes on says which object it is about (its badge) and
//! what it may do (its rights); the request never names either. A handle `fs`
//! hands back (from [`Request::Open`]) moves with the reply.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use minicbor::{Decode, Decoder, Encode, Encoder};

/// The protocol version every message starts with.
pub const VERSION: u32 = 1;
/// Largest request or reply: one IPC message.
pub const MAX_MESSAGE: usize = 8192;
/// The most file data in one [`Request::Write`] or [`Reply::Data`].
pub const MAX_DATA: usize = 7 * 1024;
/// The most names in one [`Reply::Listed`]: 24 of 255 bytes still fit.
pub const MAX_LIST: u32 = 24;
/// The most rows in one [`Reply::Changes`], [`Reply::History`] or
/// [`Reply::Restored`]; a longer answer ends with `more`.
pub const MAX_ROWS: u32 = 64;
/// The most path text in one [`Reply::Trash`]: items stop before it is passed,
/// and a single path longer than this is shown by its last this-many bytes.
pub const MAX_TRASH_TEXT: usize = 6 * 1024;
/// The most items in one [`Reply::Trash`].
pub const MAX_TRASH_ITEMS: usize = 32;

/// How [`Request::Open`] treats the last name of its path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum How {
    /// It must exist.
    #[n(0)]
    Existing,
    /// A new empty file; refused if the name is taken (create never overwrites).
    #[n(1)]
    CreateFile,
    /// A new empty folder; refused if the name is taken.
    #[n(2)]
    Mkdir,
}

/// A request, sent on the handle it is about.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum Request {
    /// On a `Dir`: the object at `path` beneath it, as a new handle with
    /// `rights` (at most the `Dir`'s). `..` never leaves the `Dir`.
    #[n(0)]
    Open {
        #[n(0)]
        path: String,
        #[n(1)]
        rights: u32,
        #[n(2)]
        how: How,
    },
    /// The object's [`Stat`]. Needs nothing beyond the handle.
    #[n(1)]
    Stat,
    /// On a `Dir`: up to `max` names after `after`. Needs ENUMERATE.
    #[n(2)]
    List {
        #[n(0)]
        after: Option<Cursor>,
        #[n(1)]
        max: u32,
    },
    /// On a `File`: `len` bytes from `off` (short at the end). Needs READ.
    #[n(3)]
    Read {
        #[n(0)]
        off: u64,
        #[n(1)]
        len: u32,
    },
    /// On a `File`: `data` at `off`. Needs WRITE.
    #[n(4)]
    Write {
        #[n(0)]
        off: u64,
        #[n(1)]
        #[cbor(with = "minicbor::bytes")]
        data: Vec<u8>,
    },
    /// On a `File`: cut or extend it to `size` bytes. Needs WRITE.
    #[n(5)]
    Truncate {
        #[n(0)]
        size: u64,
    },
    /// On a `Dir`: renames `from` to `to`, into the `Dir` handle moved with the
    /// request if there is one, else in this one. Needs DELETE here and CREATE
    /// on the destination; refused if `to` is taken.
    #[n(6)]
    Rename {
        #[n(0)]
        from: String,
        #[n(1)]
        to: String,
    },
    /// On a `Dir`: moves `name` to its volume's trash, beside `info.cbor`.
    /// Needs DELETE.
    #[n(7)]
    Trash {
        #[n(0)]
        name: String,
    },
    /// Opens action `action`: every change until [`Request::Commit`] is one
    /// commit with one undo record. Needs WRITE. One action at a time; another
    /// caller's change meanwhile is [`Error::Busy`].
    #[n(8)]
    Begin {
        #[n(0)]
        action: u64,
    },
    /// Commits the caller's action; answered once it is durable.
    #[n(9)]
    Commit,
    /// Drops the caller's action and everything in it.
    #[n(10)]
    Abort,
    /// Undoes the action committed as generation `txn` (its
    /// [`Reply::Committed`]) in this handle's volume, never over a later
    /// change, as an action of its own (`action`), so it can be undone too.
    /// Needs WRITE and DELETE.
    #[n(11)]
    Restore {
        #[n(0)]
        txn: u64,
        #[n(1)]
        action: u64,
    },
    /// The object's earlier versions, newest first. Needs READ.
    #[n(12)]
    History,
    /// The objects in this handle's volume changed since generation `since`.
    /// Needs ENUMERATE.
    #[n(13)]
    Changes {
        #[n(0)]
        since: u64,
    },
    /// Commits whatever is pending; answers the generation.
    #[n(14)]
    Sync,
    /// Free and total 4 KiB blocks.
    #[n(15)]
    Df,
    /// The generation the caller's open action will commit as, so a change can
    /// name its own commit in that commit (the shell's log line, P2 task 12).
    /// Only inside the caller's action: outside one it is [`Error::Invalid`],
    /// in another caller's [`Error::Busy`]. Begin's lock keeps it true.
    #[n(16)]
    Generation,
    /// On a `Dir`: what its volume's trash holds from beneath this `Dir`, oldest
    /// first, after item `after` ([`TrashItem::id`]). Needs ENUMERATE.
    #[n(17)]
    TrashList {
        #[n(0)]
        after: Option<u64>,
    },
    /// On a `Dir`: moves the newest trashed item that was at `path` (beneath
    /// this `Dir`, as [`TrashItem::path`]) back there. [`Error::Exists`] if the
    /// name was taken since; [`Error::NotFound`] if its folder is gone. Needs
    /// CREATE.
    #[n(18)]
    TrashRestore {
        #[n(0)]
        path: String,
    },
    /// On a `Dir`: removes for good everything its volume's trash holds from
    /// beneath this `Dir`; [`Reply::Emptied`] says how many. No undo brings it
    /// back. Needs DELETE.
    #[n(19)]
    EmptyTrash,
    /// An `update` snapshot of the `system` volume as it is now, committed:
    /// taken before a new slot's first boot, so the old slot can boot on it if
    /// the new one fails before it commits (ADR 0008 §10). Only on the `Dir`
    /// for `/system/updates` ([`kabi::fs::SYSTEM_UPDATES`]) with WRITE;
    /// Permission on any other handle. Never inside an action, as it commits on
    /// its own: Invalid in the caller's own, Busy in another's. `home` is never
    /// in it.
    #[n(20)]
    UpdateSnapshot,
    /// Makes `update` snapshot `id` the `system` volume's live tree again, and
    /// commits: the way back after a new slot failed before it committed. On
    /// the same handle as [`Request::UpdateSnapshot`], and like it never inside
    /// an action; Invalid for an id that is not an `update` snapshot.
    #[n(21)]
    UpdateRestore {
        #[n(0)]
        id: u64,
    },
    /// Drops `update` snapshot `id`, and commits: once its update committed,
    /// or `system` was restored from it, nothing needs it again and its blocks
    /// go to the GC. On the same handle as [`Request::UpdateSnapshot`], never
    /// inside an action; Invalid for an id that is not an `update` snapshot.
    #[n(22)]
    UpdateDrop {
        #[n(0)]
        id: u64,
    },
    /// Starts `name`, `<64 hex>.gguf`, as a sealed blob of `len` bytes in the
    /// model store (P4 task 10). Only on the `Dir` for the `models` volume's
    /// root ([`kabi::fs::MODELS_ROOT`]) with WRITE, and only inside the
    /// caller's own action, which holds one blob at a time.
    #[n(23)]
    SealedBegin {
        #[n(0)]
        name: String,
        #[n(1)]
        len: u64,
    },
    /// The blob's next bytes, at most [`MAX_DATA`].
    #[n(24)]
    SealedPush {
        #[n(0)]
        #[cbor(with = "minicbor::bytes")]
        data: Vec<u8>,
    },
    /// Ends the blob once every byte is in. `fs` hashes the bytes itself:
    /// their SHA-256 must be the one the name gives, else Invalid, and their
    /// FNV-1a64 is kept for the hint. The caller then commits its action.
    #[n(25)]
    SealedFinish,
    /// Points `kai-modelhint 1` at sealed blob `name`, or clears it with none,
    /// so the next boot maps that model, or none. On the same handle as
    /// [`Request::SealedBegin`], never inside an action: `fs` writes the hint
    /// in `kai-data`'s first block, outside the filesystem. Invalid for a name
    /// that is not a sealed blob of the store.
    #[n(26)]
    ModelHint {
        #[n(0)]
        name: Option<String>,
    },
}

/// Where a listing goes on: the last name it gave, with its hash.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct Cursor {
    #[n(0)]
    pub hash: u64,
    #[n(1)]
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum Kind {
    #[n(0)]
    File,
    #[n(1)]
    Dir,
}

/// What [`Request::Stat`] tells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub struct Stat {
    #[n(0)]
    pub kind: Kind,
    /// Its object id in its volume.
    #[n(1)]
    pub object: u64,
    #[n(2)]
    pub size: u64,
    /// Nanoseconds since 1970.
    #[n(3)]
    pub mtime: u64,
    #[n(4)]
    pub crtime: u64,
    /// The generation that last changed it.
    #[n(5)]
    pub changed: u64,
}

/// One name in a folder.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct Listing {
    #[n(0)]
    pub name: String,
    #[n(1)]
    pub kind: Kind,
    #[n(2)]
    pub object: u64,
}

/// What a restore did with one object (kaifs's `Outcome`).
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum Outcome {
    #[n(0)]
    Back,
    #[n(1)]
    Moved {
        #[n(0)]
        name: String,
    },
    #[n(2)]
    Beside {
        #[n(0)]
        object: u64,
        #[n(1)]
        name: String,
    },
    #[n(3)]
    Removed,
    #[n(4)]
    Left,
}

/// One thing in the trash, as [`Request::TrashList`] tells it.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct TrashItem {
    /// Which item: its cursor for the next [`Request::TrashList`].
    #[n(0)]
    pub id: u64,
    /// Where it was, beneath the `Dir` asked.
    #[n(1)]
    pub path: String,
    #[n(2)]
    pub kind: Kind,
    /// When it was trashed, nanoseconds since 1970.
    #[n(3)]
    pub deleted: u64,
    /// The action that trashed it; 0 if none.
    #[n(4)]
    pub action: u64,
}

/// One earlier version of an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub struct Version {
    #[n(0)]
    pub snapshot: u64,
    #[n(1)]
    pub generation: u64,
    #[n(2)]
    pub stat: Stat,
}

/// Why a request failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum Error {
    /// No such object or name; also a badge whose object is gone.
    #[n(0)]
    NotFound,
    #[n(1)]
    Exists,
    #[n(2)]
    NotDir,
    #[n(3)]
    IsDir,
    #[n(4)]
    NotEmpty,
    /// A bad name, path, offset or length.
    #[n(5)]
    Invalid,
    /// The handle lacks the right the request needs.
    #[n(6)]
    Permission,
    /// Another caller's action is open.
    #[n(7)]
    Busy,
    #[n(8)]
    NoSpace,
    /// The disk cannot make writes durable, so `fs` serves it read-only.
    #[n(9)]
    ReadOnly,
    /// The disk failed or its contents did not verify; the action is dropped.
    #[n(10)]
    Io,
    /// A message `fs` cannot read: wrong version, bad CBOR or too large.
    #[n(11)]
    BadRequest,
}

/// The answer to one request.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum Reply {
    /// Done, and durable if it changed anything outside an action.
    #[n(0)]
    Done,
    /// [`Request::Open`]'s object and the badge of the handle that moves with
    /// this reply. The badge is `fs`'s own naming, of use only to the kernel,
    /// which takes no handles; it grants nothing.
    #[n(1)]
    Opened(#[n(0)] Stat, #[n(1)] u64),
    #[n(2)]
    Stat(#[n(0)] Stat),
    /// Names, and where to go on if there are more.
    #[n(3)]
    Listed {
        #[n(0)]
        names: Vec<Listing>,
        #[n(1)]
        more: Option<Cursor>,
    },
    #[n(4)]
    Data(
        #[n(0)]
        #[cbor(with = "minicbor::bytes")]
        Vec<u8>,
    ),
    /// The commit that made a change durable.
    #[n(5)]
    Committed {
        #[n(0)]
        generation: u64,
    },
    #[n(6)]
    Restored {
        #[n(0)]
        rows: Vec<(u64, Outcome)>,
        #[n(1)]
        generation: u64,
    },
    #[n(7)]
    History(#[n(0)] Vec<Version>),
    /// Changed objects, and whether there were more than fit.
    #[n(8)]
    Changes {
        #[n(0)]
        rows: Vec<Stat>,
        #[n(1)]
        more: bool,
    },
    #[n(9)]
    Space {
        #[n(0)]
        free: u64,
        #[n(1)]
        total: u64,
    },
    #[n(10)]
    Failed(#[n(0)] Error),
    /// [`Request::Generation`]'s answer.
    #[n(11)]
    Generation {
        #[n(0)]
        generation: u64,
    },
    /// Trashed items, and whether more follow the last.
    #[n(12)]
    Trash {
        #[n(0)]
        items: Vec<TrashItem>,
        #[n(1)]
        more: bool,
    },
    /// [`Request::EmptyTrash`]'s answer: how many items went for good.
    #[n(13)]
    Emptied {
        #[n(0)]
        count: u64,
    },
    /// [`Request::UpdateSnapshot`]'s answer: the snapshot, and the commit that
    /// holds it.
    #[n(14)]
    Snapshot {
        #[n(0)]
        id: u64,
        #[n(1)]
        generation: u64,
    },
}

/// `[VERSION, body]` as bytes.
fn encode<T: Encode<()>>(body: &T) -> Vec<u8> {
    let mut out = Vec::new();
    let mut e = Encoder::new(&mut out);
    // Writing to a Vec cannot fail.
    let _ = e
        .array(2)
        .and_then(|e| e.u32(VERSION))
        .and_then(|e| e.encode(body));
    out
}

/// The body of `[VERSION, body]`, or [`Error::BadRequest`] for anything else:
/// another version, bad CBOR, trailing bytes or more than [`MAX_MESSAGE`].
fn decode<'b, T: Decode<'b, ()>>(bytes: &'b [u8]) -> Result<T, Error> {
    if bytes.len() > MAX_MESSAGE {
        return Err(Error::BadRequest);
    }
    let mut d = Decoder::new(bytes);
    let ok = d.array().ok() == Some(Some(2)) && d.u32().ok() == Some(VERSION);
    if !ok {
        return Err(Error::BadRequest);
    }
    let body = d.decode().map_err(|_| Error::BadRequest)?;
    if d.position() != bytes.len() {
        return Err(Error::BadRequest);
    }
    Ok(body)
}

pub fn encode_request(request: &Request) -> Vec<u8> {
    encode(request)
}

pub fn decode_request(bytes: &[u8]) -> Result<Request, Error> {
    decode(bytes)
}

pub fn encode_reply(reply: &Reply) -> Vec<u8> {
    encode(reply)
}

pub fn decode_reply(bytes: &[u8]) -> Result<Reply, Error> {
    decode(bytes)
}

#[cfg(test)]
mod tests;
