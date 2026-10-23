//! Services: serving requests, and calling another service.

use super::{MAX_BIND_WAIT_NS, MAX_CALL_MS, NESTED_MARGIN_NS, call_error, need};
use crate::process::uaccess;
use crate::{hpet, ipc};
use alloc::vec::Vec;
use kabi::{Error, call, cap};

/// The calls this area serves.
const CALLS: &[u64] = &[call::SVC_RECV, call::SVC_REPLY, call::CALL];

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
        call::CALL => {
            need(slot, cap::CALL)?;
            service_call(slot, a[0], a[1], a[2])
        }
        _ => Err(Error::NotFound),
    }
}

/// Calls another service on behalf of the process. The target is named by the
/// caller, but may not be the caller itself nor a service that can itself call, so
/// calls never form a cycle and a service with CALL cannot be borrowed by another.
fn service_call(slot: usize, name_ptr: u64, name_len: u64, request_ptr: u64) -> Result<u64, Error> {
    let name_len = usize::try_from(name_len).map_err(|_| Error::Invalid)?;
    if name_len == 0 || name_len > kabi::MAX_NAME {
        return Err(Error::Invalid);
    }
    let mut name = [0u8; kabi::MAX_NAME];
    uaccess::copy_from_user(slot, name_ptr, &mut name[..name_len])?;
    let name = core::str::from_utf8(&name[..name_len]).map_err(|_| Error::Invalid)?;
    let me = crate::process::identity(slot);
    if name == me.name.as_str() {
        return Err(Error::Invalid);
    }
    if crate::process::programs::find(name).is_none() && !crate::process::supervision::planned(name)
    {
        return Err(Error::NotFound);
    }
    if crate::process::has_cap_by_name(name, cap::CALL) {
        return Err(Error::Permission);
    }
    // Only the kernel drives the publisher, whether or not it is running now.
    if name == crate::process::supervision::PUBLISHER
        || crate::process::has_cap_by_name(name, cap::PUBLISH)
    {
        return Err(Error::Permission);
    }
    let call: kabi::CallRequest = uaccess::read_struct(slot, request_ptr)?;
    let request_len = usize::try_from(call.request_len).map_err(|_| Error::TooBig)?;
    if request_len > kabi::MAX_MESSAGE {
        return Err(Error::TooBig);
    }
    let mut request = Vec::new();
    request
        .try_reserve_exact(request_len)
        .map_err(|_| Error::NoMemory)?;
    request.resize(request_len, 0);
    uaccess::copy_from_user(slot, call.request_ptr, &mut request)?;
    let now = hpet::nanos();
    let mut deadline = now + call.timeout_ms.clamp(1, MAX_CALL_MS) * 1_000_000;
    if let Some(served) = ipc::serving_deadline(me.pid) {
        deadline = deadline.min(served.saturating_sub(NESTED_MARGIN_NS));
    }
    let cancel = || crate::process::killed(slot) || crate::process::interrupt_pending(slot);
    let result = ipc::wait_bound(name, deadline.min(now + MAX_BIND_WAIT_NS), cancel)
        .and_then(|()| ipc::call_as(name, &request, me.pid, deadline, cancel));
    drop(request);
    let reply = result.map_err(|error| call_error(slot, error))?;
    let len = reply
        .len()
        .min(usize::try_from(call.reply_cap).unwrap_or(usize::MAX));
    uaccess::copy_to_user(slot, call.reply_ptr, &reply[..len])?;
    Ok(reply.len() as u64)
}
