//! `updated`'s two folders on `kai-data`, from its plan line's `dir=trust+updates`
//! (ADR 0008 §7, §10): `/system/trust` keeps the TUF metadata `kaituf` verified,
//! so the next refresh starts from the newest root and refuses replayed
//! metadata; `/system/updates` is the handle the `update` snapshot of `system`
//! is taken on before a new slot's first boot. Without `kai-data` neither
//! exists, and `updated` keeps its metadata in memory for the one refresh.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use fsproto::{How, MAX_DATA, Reply, Request};
use kabi::Error;
use kabi::handle::{Handle, HandleInfo, Kind, MAX_STARTUP, MAX_TRANSFER, Rights};
use user::sys;

const CALL_MS: u64 = 10_000;

/// The startup `Dir`s, in the order the plan line names them.
pub struct Folders {
    pub trust: Option<Handle>,
    pub updates: Option<Handle>,
}

impl Folders {
    /// A `Dir` with no `fs` behind it (no `kai-data` on this machine: the call
    /// is Unavailable), or one `fs` refuses (mounted read-only without the
    /// folders), is none. An `fs` that is there but does not answer is an
    /// error: guessing "none" would skip the cache and the snapshot.
    pub fn from_startup() -> Result<Folders, String> {
        let mut handles = [HandleInfo::default(); MAX_STARTUP];
        let count = sys::startup(&mut handles).unwrap_or(0);
        let mut dirs = handles[..count.min(MAX_STARTUP)]
            .iter()
            .filter(|h| h.kind() == Some(Kind::Dir))
            .map(|h| Handle(h.handle));
        let mut served = || -> Result<Option<Handle>, String> {
            let Some(dir) = dirs.next() else {
                return Ok(None);
            };
            if !served_by_fs(dir) {
                return Ok(None);
            }
            match ask(dir, &Request::Stat)? {
                (Reply::Stat(_), _) => Ok(Some(dir)),
                (Reply::Failed(_), _) => Ok(None),
                other => Err(format!("fs: Stat answered {other:?}")),
            }
        };
        Ok(Folders {
            trust: served()?,
            updates: served()?,
        })
    }
}

/// Another caller's open action makes a change Busy for a moment: try again
/// for up to `BUSY_TRIES * BUSY_MS` before giving up.
const BUSY_TRIES: u32 = 50;
const BUSY_MS: u64 = 100;

fn ask(on: Handle, request: &Request) -> Result<(Reply, Option<Handle>), String> {
    for _ in 1..BUSY_TRIES {
        match ask_once(on, request)? {
            (Reply::Failed(fsproto::Error::Busy), _) => sys::sleep_ms(BUSY_MS),
            answer => return Ok(answer),
        }
    }
    ask_once(on, request)
}

/// Tries before a `Dir` with no server counts as none: an `fs` the supervisor
/// is restarting is back within a few tens of milliseconds.
const NO_SERVER_TRIES: u32 = 5;
const NO_SERVER_MS: u64 = 200;

/// Whether a server answers on `dir` at all: the kernel says Unavailable when
/// none does.
fn served_by_fs(dir: Handle) -> bool {
    let bytes = fsproto::encode_request(&Request::Stat);
    let mut reply = vec![0u8; fsproto::MAX_MESSAGE];
    let mut moved = [Handle::NONE; MAX_TRANSFER];
    for _ in 1..NO_SERVER_TRIES {
        let call = sys::call_h(dir, &bytes, &mut reply, CALL_MS, &[], &mut moved);
        if call != Err(Error::Unavailable) {
            return true;
        }
        sys::sleep_ms(NO_SERVER_MS);
    }
    sys::call_h(dir, &bytes, &mut reply, CALL_MS, &[], &mut moved) != Err(Error::Unavailable)
}

fn ask_once(on: Handle, request: &Request) -> Result<(Reply, Option<Handle>), String> {
    let bytes = fsproto::encode_request(request);
    let mut reply = vec![0u8; fsproto::MAX_MESSAGE];
    let mut moved = [Handle::NONE; MAX_TRANSFER];
    let (len, came) = sys::call_h(on, &bytes, &mut reply, CALL_MS, &[], &mut moved)
        .map_err(|e| format!("fs: {e:?}"))?;
    let reply = fsproto::decode_reply(&reply[..len]).map_err(|e| format!("fs reply: {e:?}"))?;
    Ok((reply, (came > 0).then_some(moved[0])))
}

fn open(dir: Handle, name: &str, how: How) -> Result<Option<Handle>, String> {
    let request = Request::Open {
        path: name.into(),
        rights: (Rights::READ | Rights::WRITE).bits(),
        how,
    };
    match ask(dir, &request)? {
        (Reply::Opened(..), Some(file)) => Ok(Some(file)),
        (Reply::Failed(fsproto::Error::NotFound), _) => Ok(None),
        other => Err(format!("opening {name}: {other:?}")),
    }
}

/// The whole of `name` in `dir`, or `None` if there is no such file.
pub fn read(dir: Handle, name: &str) -> Result<Option<Vec<u8>>, String> {
    let Some(file) = open(dir, name, How::Existing)? else {
        return Ok(None);
    };
    let mut out = Vec::new();
    loop {
        let request = Request::Read {
            off: out.len() as u64,
            len: MAX_DATA as u32,
        };
        match ask(file, &request)? {
            (Reply::Data(bytes), _) if bytes.is_empty() => break,
            (Reply::Data(bytes), _) => out.extend_from_slice(&bytes),
            other => return Err(format!("reading {name}: {other:?}")),
        }
    }
    let _ = sys::handle_close(file);
    Ok(Some(out))
}

/// Replaces `name` in `dir` with `bytes`, in one `fs` action so a reader never
/// sees half of it.
pub fn write(dir: Handle, name: &str, bytes: &[u8]) -> Result<(), String> {
    let done = |what: &str, reply: Result<(Reply, Option<Handle>), String>| match reply? {
        (Reply::Done | Reply::Committed { .. }, _) => Ok(()),
        other => Err(format!("{what} {name}: {other:?}")),
    };
    done("beginning", ask(dir, &Request::Begin { action: 0 }))?;
    let result = (|| {
        let file = match open(dir, name, How::Existing)? {
            Some(file) => file,
            None => open(dir, name, How::CreateFile)?.ok_or_else(|| format!("no {name}"))?,
        };
        done("truncating", ask(file, &Request::Truncate { size: 0 }))?;
        for (n, chunk) in bytes.chunks(MAX_DATA).enumerate() {
            let request = Request::Write {
                off: (n * MAX_DATA) as u64,
                data: chunk.into(),
            };
            match ask(file, &request)? {
                (Reply::Done, _) => {}
                other => return Err(format!("writing {name}: {other:?}")),
            }
        }
        let _ = sys::handle_close(file);
        Ok(())
    })();
    match result {
        Ok(()) => done("committing", ask(dir, &Request::Commit)),
        Err(why) => {
            let _ = ask(dir, &Request::Abort);
            Err(why)
        }
    }
}

/// Makes `update` snapshot `id` the `system` volume's live tree again.
/// What [`update_restore`] did.
pub enum Restore {
    Done,
    /// No update snapshot `id` (fs answers Invalid).
    Gone,
}

pub fn update_restore(updates: Handle, id: u64) -> Result<Restore, String> {
    match ask(updates, &Request::UpdateRestore { id })? {
        (Reply::Committed { .. }, _) => Ok(Restore::Done),
        (Reply::Failed(fsproto::Error::Invalid), _) => Ok(Restore::Gone),
        other => Err(format!("restoring update snapshot {id}: {other:?}")),
    }
}

/// Drops `update` snapshot `id` once nothing needs it.
pub fn update_drop(updates: Handle, id: u64) -> Result<(), String> {
    match ask(updates, &Request::UpdateDrop { id })? {
        (Reply::Committed { .. }, _) => Ok(()),
        other => Err(format!("dropping update snapshot {id}: {other:?}")),
    }
}

/// An `update` snapshot of the `system` volume, on `/system/updates`.
pub fn update_snapshot(updates: Handle) -> Result<u64, String> {
    match ask(updates, &Request::UpdateSnapshot)? {
        (Reply::Snapshot { id, .. }, _) => Ok(id),
        other => Err(format!("the update snapshot: {other:?}")),
    }
}
