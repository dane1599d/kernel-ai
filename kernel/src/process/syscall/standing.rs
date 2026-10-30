//! Standing instructions' calls (P3 task 11): `desktop` posts window and idle
//! events into [`crate::event_box`] (EVENT_POST), the shell takes them
//! (EVENT_READ), and the supervisor runs a firing's `app_suspend` or
//! `app_resume` through the kernel's own `ctl` request (APP_CTL), after its
//! RULE_CHECK. The supervisor also forwards the agent core's calls to apps'
//! actions the same way (APP_CALL, P4).

use super::need;
use crate::process::uaccess::{copy_from_user, copy_to_user};
use crate::{event_box, ipc, process};
use kabi::event::{Event, Kind, MAX_LINE, READ_BYTES};
use kabi::{Error, call, cap};

const CALLS: &[u64] = &[
    call::EVENT_POST,
    call::EVENT_READ,
    call::APP_CTL,
    call::APP_CALL,
];

/// How long a suspend or resume may take: saving and restoring the app's state.
const CTL_MS: u64 = 60_000;
/// The longest an app's call may take: the supervisor waits on it, so a slow
/// or hung app cannot hold it longer (`supervise::appcall::WAIT_MS`).
const APP_CALL_MS: u64 = 5_000;
/// The requests APP_CALL forwards, by their first line, and the one it
/// forwards whole: `kai-actions 1`, the app's rows.
const APP_REQUESTS: &[&[u8]] = &[b"kai-call 1\n", b"kai-undo 1\n"];
const APP_ROWS: &[&[u8]] = &[b"kai-actions 1", b"kai-actions 1\n"];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| match number {
        call::EVENT_POST => post(slot, a[0], a[1]),
        call::EVENT_READ => read(slot, a[0], a[1]),
        call::APP_CALL => app_call(slot, a),
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

/// The app name at `ptr`: a valid service name.
fn app_name(slot: usize, ptr: u64, len: u64, buf: &mut [u8]) -> Result<&str, Error> {
    let len = line_from(slot, ptr, len, buf)?;
    let name = core::str::from_utf8(&buf[..len]).map_err(|_| Error::Invalid)?;
    match kabi::event::valid_name(name) {
        true => Ok(name),
        false => Err(Error::Invalid),
    }
}

/// APP_CALL: a `kai-call 1` or `kai-undo 1` to app `name`, as the kernel, for
/// the supervisor. Only a process holding `component` is an app; one not
/// serving is NotFound at once, so no wait holds the supervisor.
fn app_call(slot: usize, a: &[u64; 6]) -> Result<u64, Error> {
    need(slot, cap::SUPERVISE)?;
    let mut name = [0u8; kabi::event::MAX_NAME];
    let name = app_name(slot, a[0], a[1], &mut name)?;
    if !process::has_cap_by_name(name, cap::COMPONENT) {
        return Err(Error::NotFound);
    }
    let len = usize::try_from(a[3]).map_err(|_| Error::TooBig)?;
    if len > kabi::MAX_MESSAGE {
        return Err(Error::TooBig);
    }
    let mut request = alloc::vec![0u8; len];
    copy_from_user(slot, a[2], &mut request)?;
    if !APP_REQUESTS.iter().any(|head| request.starts_with(head))
        && !APP_ROWS.contains(&request.as_slice())
    {
        return Err(Error::Invalid);
    }
    ipc::server_of(name).ok_or(Error::NotFound)?;
    // The reply's room in the low half, the time the caller has left in the high.
    let cap = a[5] & u64::from(u32::MAX);
    let wait_ms = match a[5] >> 32 {
        0 => APP_CALL_MS,
        ms => ms.min(APP_CALL_MS),
    };
    let reply = ipc::call(name, &request, wait_ms).map_err(|error| match error {
        ipc::CallError::Unavailable => Error::NotFound,
        ipc::CallError::NoMemory => Error::NoMemory,
        _ => Error::TimedOut,
    })?;
    let reply = &reply[..reply.len().min(usize::try_from(cap).unwrap_or(usize::MAX))];
    copy_to_user(slot, a[4], reply)?;
    Ok(reply.len() as u64)
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
