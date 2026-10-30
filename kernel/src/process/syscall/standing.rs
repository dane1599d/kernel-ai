//! Standing instructions' calls (P3 task 11): `desktop` posts window and idle
//! events into [`crate::event_box`] (EVENT_POST), the shell takes them
//! (EVENT_READ), and the supervisor runs a firing's `app_suspend` or
//! `app_resume` through the kernel's own `ctl` request (APP_CTL), after its
//! RULE_CHECK.

use super::need;
use crate::process::uaccess::{copy_from_user, copy_to_user};
use crate::{event_box, ipc, process};
use kabi::event::{Event, Kind, MAX_LINE, READ_BYTES};
use kabi::{Error, call, cap};

const CALLS: &[u64] = &[call::EVENT_POST, call::EVENT_READ, call::APP_CTL];

/// How long a suspend or resume may take: saving and restoring the app's state.
const CTL_MS: u64 = 60_000;

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| match number {
        call::EVENT_POST => post(slot, a[0], a[1]),
        call::EVENT_READ => read(slot, a[0], a[1]),
        _ => app_ctl(slot, a[0], a[1], a[2], a[3], a[4]),
    })
}

/// Copies a short line from the caller, as UTF-8.
fn line_from(slot: usize, ptr: u64, len: u64, buf: &mut [u8]) -> Result<usize, Error> {
    let len = usize::try_from(len).map_err(|_| Error::TooBig)?;
    if len == 0 || len > buf.len() {
        return Err(Error::TooBig);
    }
    copy_from_user(slot, ptr, &mut buf[..len])?;
    core::str::from_utf8(&buf[..len]).map_err(|_| Error::Invalid)?;
    Ok(len)
}

/// EVENT_POST: one event from the display holder.
fn post(slot: usize, ptr: u64, len: u64) -> Result<u64, Error> {
    need(slot, cap::DISPLAY)?;
    let mut buf = [0u8; MAX_LINE];
    let len = line_from(slot, ptr, len, &mut buf)?;
    let line = core::str::from_utf8(&buf[..len]).map_err(|_| Error::Invalid)?;
    let event = Event::parse(line).ok_or(Error::Invalid)?;
    // The time of day is the kernel's own.
    if let Kind::Time(_) = event.kind {
        return Err(Error::Invalid);
    }
    event_box::post(event);
    Ok(0)
}

/// EVENT_READ: every waiting event, for the process serving `shell` only.
fn read(slot: usize, buf_ptr: u64, cap_bytes: u64) -> Result<u64, Error> {
    let pid = process::identity(slot).pid;
    if ipc::server_of("shell") != Some(pid) {
        return Err(Error::Permission);
    }
    if cap_bytes < READ_BYTES as u64 {
        return Err(Error::TooBig);
    }
    let (dropped, events) = event_box::take_for(pid);
    let text = event_box::text(dropped, &events);
    copy_to_user(slot, buf_ptr, text.as_bytes())?;
    Ok(text.len() as u64)
}

/// APP_CTL: `ctl suspend`, `ctl resume` or `ctl keep-running on|off` to app
/// `name`, as the kernel.
fn app_ctl(slot: usize, ptr: u64, len: u64, verb: u64, buf: u64, cap: u64) -> Result<u64, Error> {
    need(slot, cap::SUPERVISE)?;
    let request: &[u8] = match verb {
        0 => b"ctl suspend",
        1 => b"ctl resume",
        2 => b"ctl keep-running on",
        3 => b"ctl keep-running off",
        _ => return Err(Error::Invalid),
    };
    let mut name = [0u8; kabi::event::MAX_NAME];
    let len = line_from(slot, ptr, len, &mut name)?;
    let name = core::str::from_utf8(&name[..len]).map_err(|_| Error::Invalid)?;
    if !kabi::event::valid_name(name) {
        return Err(Error::Invalid);
    }
    ipc::server_of(name).ok_or(Error::NotFound)?;
    let reply = ipc::call(name, request, CTL_MS).map_err(|error| match error {
        ipc::CallError::Unavailable => Error::NotFound,
        ipc::CallError::NoMemory => Error::NoMemory,
        _ => Error::TimedOut,
    })?;
    let reply = &reply[..reply.len().min(usize::try_from(cap).unwrap_or(usize::MAX))];
    copy_to_user(slot, buf, reply)?;
    Ok(reply.len() as u64)
}
