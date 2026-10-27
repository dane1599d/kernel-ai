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
