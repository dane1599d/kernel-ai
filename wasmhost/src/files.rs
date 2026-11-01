//! `kai:files@0.1.0` for `lifecycle 2` (P4): a granted interface through which the
//! editor reads and writes a file of the person's own. `read` and `write` only
//! queue the request, as `kai:edit` queues its messages, because an app's call is
//! transactional and can run on two versions. comphost takes the queue after the
//! call ([`CoreInstance::take_files`]), has the OS do each and gives the app the
//! answer in a later call, `replies.on-done` ([`CoreInstance::files_reply`]). So
//! a call never waits on the disk, and a version rehearsed beside the active one
//! cannot write twice while it runs: only the shown version's queue is taken.
//!
//! The interface names a whole file and says nothing of how it travels. A request
//! says whether a person's key or click was being handled when the app sent it
//! ([`FilesRequest::user`]), which comphost and the OS read before they pass a
//! write on.
//!
//! The interface is linked only for an instance loaded with the `files` grant
//! ([`Grants`]); without it the import fails to link, and the version does not
//! stage (`Reason::Link`).

use crate::canonical::CoreInstance;
use crate::instance::HostState;
use crate::tables;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use appabi::Val;
use livecore::{Fault, Reason};
use wasmi::{Caller, Error, Extern};

/// The interface whose functions this module serves.
pub(crate) const FILES: &str = "kai:files/files";
/// The export the answers go to.
pub(crate) const REPLIES: &str = "kai:files/replies";

/// Longest path a request names.
pub const MAX_PATH: usize = 255;
/// Most bytes of a file a write carries (and a read answers).
pub const MAX_DATA: usize = 256 * 1024;
/// Most requests waiting for comphost, which takes them after every call it
/// makes into the app; one that would be over this traps the call.
pub const MAX_QUEUED: usize = 2;

/// What the app asked of the OS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileOp {
    /// The whole file at `path`.
    Read { path: String },
    /// `data` as the whole text of the file at `path`: made (it must not exist)
    /// when `create`, else replaced.
    Write {
        path: String,
        data: Vec<u8>,
        create: bool,
    },
}

/// A request the app sent, waiting for comphost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilesRequest {
    /// What `on-done` is told the answer is for.
    pub ticket: u32,
    pub op: FileOp,
    /// Whether a person's key or click was being handled when the app sent it
    /// ([`CoreInstance::ui_input`], [`CoreInstance::ui_events_user`]), and not an
    /// answer, an event, the agent's `ui_act` or a timer. A write is passed on
    /// only for a request that is.
    pub user: bool,
}

/// The requests not yet taken, and the next ticket.
#[derive(Default)]
pub(crate) struct FilesHost {
    pub(crate) queue: Vec<FilesRequest>,
    next: u32,
}

/// Why `path` is not one a request may name: absolute, plain segments, no
/// whitespace, colon, backslash or control character, at most [`MAX_PATH`] bytes.
pub fn check_path(path: &str) -> Result<(), String> {
    let plain = path
        .strip_prefix('/')
        .ok_or_else(|| alloc::format!("{path:?} is not an absolute path"))?;
    if path.len() > MAX_PATH {
        return Err(alloc::format!("the path is over {MAX_PATH} bytes"));
    }
    livecore::artifact::valid_path(plain).map_err(|why| alloc::format!("{path}: {why}"))
}

/// A string of `len` bytes at `ptr` in the app's memory.
fn bytes_at(caller: &mut Caller<'_, HostState>, ptr: i32, len: i32) -> Result<Vec<u8>, Error> {
    let start = ptr as u32 as usize;
    let len = len as u32 as usize;
    caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .and_then(|memory| {
            memory
                .data(&*caller)
                .get(start..start.saturating_add(len))
                .map(<[u8]>::to_vec)
        })
        .ok_or_else(|| Error::new("bytes outside memory"))
}

fn path_at(
    caller: &mut Caller<'_, HostState>,
    ptr: i32,
    len: i32,
    what: &str,
) -> Result<String, Error> {
    let bytes = bytes_at(caller, ptr, len)?;
    let path = String::from_utf8(bytes)
        .map_err(|_| Error::new(alloc::format!("{FILES} {what}: the path is not text")))?;
    check_path(&path).map_err(|why| Error::new(alloc::format!("{FILES} {what}: {why}")))?;
    Ok(path)
}

/// Queues `op` if the call is a transaction and the queue has room: its ticket.
fn queue(caller: &mut Caller<'_, HostState>, what: &str, op: FileOp) -> Result<i32, Error> {
    // Only inside a transaction (an action, a window's input, an event), as for
    // storage and effects: a probation's `init`, `health` or `self-test` cannot
    // touch a file by itself.
    if caller.data().call.is_none() {
        return Err(Error::new(alloc::format!(
            "{FILES} {what}: only inside a call that a transaction wraps"
        )));
    }
    let user = caller.data().user_input;
    let host = &mut caller.data_mut().files;
    if host.queue.len() >= MAX_QUEUED {
        return Err(Error::new(alloc::format!(
            "{FILES} {what}: {MAX_QUEUED} requests queued before comphost took them"
        )));
    }
    let ticket = host.next;
    host.next = host.next.wrapping_add(1);
    host.queue.push(FilesRequest { ticket, op, user });
    Ok(ticket as i32)
}

/// `files.read(path) -> u32`: the request's ticket.
pub(crate) fn read(mut caller: Caller<'_, HostState>, ptr: i32, len: i32) -> Result<i32, Error> {
    let path = path_at(&mut caller, ptr, len, "read")?;
    queue(&mut caller, "read", FileOp::Read { path })
}

/// `files.write(path, data, create) -> u32`: the request's ticket.
pub(crate) fn write(
    mut caller: Caller<'_, HostState>,
    path_ptr: i32,
    path_len: i32,
    data_ptr: i32,
    data_len: i32,
    create: i32,
) -> Result<i32, Error> {
    let path = path_at(&mut caller, path_ptr, path_len, "write")?;
    if data_len as u32 as usize > MAX_DATA {
        return Err(Error::new(alloc::format!(
            "{FILES} write: {} bytes; at most {MAX_DATA}",
            data_len as u32
        )));
    }
    let data = bytes_at(&mut caller, data_ptr, data_len)?;
    let op = FileOp::Write {
        path,
        data,
        create: create != 0,
    };
    queue(&mut caller, "write", op)
}

impl CoreInstance {
    /// The requests the app sent since the last call here, oldest first.
    pub fn take_files(&mut self) -> Vec<FilesRequest> {
        core::mem::take(&mut self.store.data_mut().files.queue)
    }

    /// Tells the app the answer to `ticket`: a read's bytes, or why there is
    /// none. An app that does not export `kai:files/replies` is told nothing.
    pub fn files_reply(
        &mut self,
        ticket: u32,
        outcome: Result<Vec<u8>, String>,
    ) -> Result<(), Fault> {
        let Some(export) = self.on_files_reply.clone() else {
            return Ok(());
        };
        let entry = tables::EXPORTS
            .iter()
            .find(|f| f.interface == REPLIES && f.name == "on-done")
            .ok_or_else(|| Fault::new(Reason::Host, "no kai:files/replies table"))?;
        let outcome = match outcome {
            Ok(bytes) => Val::Result(Ok(Some(Box::new(Val::Bytes(bytes))))),
            Err(why) => Val::Result(Err(Some(Box::new(Val::String(why))))),
        };
        livecore::Instance::call(self, &entry.func(export), &[Val::U32(ticket), outcome])
            .map(|_| ())
    }
}
