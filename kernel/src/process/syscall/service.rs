//! Services: serving requests, and calling another service through a route
//! handle (ADR 0001 D3, step 4). Calling by name is retired: CALL answers
//! NotFound, and a process reaches only the services its plan line names in
//! `uses=`, through the `Service` routes it got at spawn.

use super::{MAX_BIND_WAIT_NS, MAX_CALL_MS, NESTED_MARGIN_NS, call_error, need};
use crate::handle;
use crate::process::uaccess;
use crate::{hpet, ipc};
use alloc::vec::Vec;
use kabi::handle::{CallH, Handle, Kind, MAX_TRANSFER, RecvInfo, Rights};
use kabi::{Error, call, cap};

/// The calls this area serves.
const CALLS: &[u64] = &[
    call::SVC_RECV,
    call::SVC_REPLY,
    call::CALL,
    call::CALL_H,
    call::RECV_H,
    call::REPLY_H,
];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| run(slot, number, a))
}

fn run(slot: usize, number: u64, a: &[u64; 6]) -> Result<u64, Error> {
    match number {
        call::SVC_RECV => {
            need(slot, cap::SERVE)?;
            ipc::recv(slot, a[0], a[1], a[2], a[3])
        }
        call::SVC_REPLY => {
            need(slot, cap::SERVE)?;
            ipc::reply(slot, a[0], a[1])
        }
        call::RECV_H => {
            need(slot, cap::SERVE)?;
            recv_h(slot, a[0], a[1], a[2], a[3])
        }
        call::REPLY_H => {
            need(slot, cap::SERVE)?;
            reply_h(slot, a[0], a[1], a[2], a[3])
        }
        call::CALL_H => call_h(slot, a[0], a[1]),
        _ => Err(Error::NotFound),
    }
}

/// Up to [`MAX_TRANSFER`] handle values from ring 3.
fn read_handles(slot: usize, ptr: u64, len: u64) -> Result<Vec<Handle>, Error> {
    let len = usize::try_from(len).map_err(|_| Error::TooBig)?;
    if len > MAX_TRANSFER {
        return Err(Error::TooBig);
    }
    let mut raw = [0u8; 4 * MAX_TRANSFER];
    uaccess::copy_from_user(slot, ptr, &mut raw[..4 * len])?;
    let (words, _) = raw[..4 * len].as_chunks::<4>();
    Ok(words
        .iter()
        .map(|word| Handle(u32::from_le_bytes(*word)))
        .collect())
}

/// CALL_H: sends a request through the `Service` route `route` and waits for the
/// reply, moving handles each way.
fn call_h(slot: usize, route: u64, args_ptr: u64) -> Result<u64, Error> {
    let route = u32::try_from(route)
        .ok()
        .filter(|&r| r != 0)
        .map(Handle)
        .ok_or(Error::BadHandle)?;
    let entry = handle::with(slot, |h| h.table.check(route, Kind::Service, Rights::CALL))
        .map_err(|_| Error::BadHandle)?
        .map_err(Error::from)?;
    let name = handle::route_name(entry.object).ok_or(Error::NotFound)?;
    let name = name.as_str();
    let args: CallH = uaccess::read_struct(slot, args_ptr)?;
    let request_len = usize::try_from(args.request_len).map_err(|_| Error::TooBig)?;
    if request_len > kabi::MAX_MESSAGE {
        return Err(Error::TooBig);
    }
    let out_cap = usize::try_from(args.handles_out_cap)
        .unwrap_or(usize::MAX)
        .min(MAX_TRANSFER);
    let sending = read_handles(slot, args.handles_ptr, args.handles_len)?;
    let mut request = Vec::new();
    request
        .try_reserve_exact(request_len)
        .map_err(|_| Error::NoMemory)?;
    request.resize(request_len, 0);
    uaccess::copy_from_user(slot, args.request_ptr, &mut request)?;
    let me = crate::process::identity(slot);
    let now = hpet::nanos();
    let mut deadline = now + args.timeout_ms.clamp(1, MAX_CALL_MS) * 1_000_000;
    if let Some(served) = ipc::serving_deadline(me.pid) {
        deadline = deadline.min(served.saturating_sub(NESTED_MARGIN_NS));
    }
    let cancel = || crate::process::killed(slot) || crate::process::interrupt_pending(slot);
    ipc::wait_bound(name, deadline.min(now + MAX_BIND_WAIT_NS), cancel)
        .map_err(|error| call_error(slot, error))?;
    // The handles go only to a server whose capability bits allow their kinds.
    let moved = match sending.is_empty() {
        true => Vec::new(),
        false => {
            let server = ipc::server_of(name).and_then(handle::caps_of);
            handle::take_for(slot, &sending, server.ok_or(Error::Unavailable)?)?
        }
    };
    let extra = ipc::Extra {
        badge: entry.badge,
        rights: entry.rights.bits(),
        handles: moved,
    };
    let result = ipc::call_with(name, &request, me.pid, deadline, cancel, extra);
    drop(request);
    let (reply, _, came) = result.map_err(|error| call_error(slot, error))?;
    let len = reply
        .len()
        .min(usize::try_from(args.reply_cap).unwrap_or(usize::MAX));
    uaccess::copy_to_user(slot, args.reply_ptr, &reply[..len])?;
    let mut given = handle::give(slot, came);
    // Handles past the room the caller gave are closed, not leaked.
    for extra in given.split_off(given.len().min(out_cap)) {
        let _ = handle::with(slot, |h| h.table.close(extra));
    }
    given.resize(out_cap, Handle::NONE);
    let raw: Vec<u8> = given.iter().flat_map(|h| h.0.to_le_bytes()).collect();
    uaccess::copy_to_user(slot, args.handles_out_ptr, &raw)?;
    Ok(reply.len() as u64)
}

/// RECV_H: the next request, with who sent it, its route's badge and rights,
/// and the handles it moved, now in this process's table.
fn recv_h(slot: usize, buf: u64, cap: u64, timeout_ms: u64, info_ptr: u64) -> Result<u64, Error> {
    let (len, caller_pid, extra) = ipc::recv_with(slot, buf, cap, timeout_ms)?;
    let given = handle::give(slot, extra.handles);
    let mut info = RecvInfo {
        caller_pid,
        badge: extra.badge,
        rights: extra.rights,
        handle_count: given.len() as u32,
        handles: [0; MAX_TRANSFER],
    };
    for (out, handle) in info.handles.iter_mut().zip(&given) {
        *out = handle.0;
    }
    // SAFETY: RecvInfo is `repr(C)` plain old data with no padding.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&raw const info).cast::<u8>(),
            core::mem::size_of::<RecvInfo>(),
        )
    };
    uaccess::copy_to_user(slot, info_ptr, bytes)?;
    Ok(len)
}

/// REPLY_H: answers the call taken by the last receive, moving handles to its
/// caller, which must be a process whose capability bits allow their kinds.
fn reply_h(
    slot: usize,
    ptr: u64,
    len: u64,
    handles_ptr: u64,
    handles_len: u64,
) -> Result<u64, Error> {
    let sending = read_handles(slot, handles_ptr, handles_len)?;
    let moved = match sending.is_empty() {
        true => Vec::new(),
        false => {
            // The kernel takes no handles.
            let caller = handle::caps_of(ipc::current_caller(slot)?).ok_or(Error::Invalid)?;
            handle::take_for(slot, &sending, caller)?
        }
    };
    ipc::reply_with(slot, ptr, len, moved)
}
