//! Live components and the build service.

use super::need;
use super::net::{Destination, exchange_to, read_http};
use crate::process::uaccess;
use kabi::{Error, call, cap};

/// The calls this area serves.
const CALLS: &[u64] = &[
    call::COMP_FETCH,
    call::COMP_EVENT,
    call::COMP_RESTORE,
    call::COMP_PUT,
    call::COMP_READ,
    call::BUILD,
    call::BUILD_START,
    call::BUILD_POLL,
    call::COMP_CONTROL,
    call::EDIT_CALL,
];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| run(slot, number, a))
}

fn run(slot: usize, number: u64, a: &[u64; 6]) -> Result<u64, Error> {
    match number {
        call::COMP_FETCH => {
            need(slot, cap::COMPONENT)?;
            crate::process::components::fetch(slot, a[0], a[1], a[2], a[3])
        }
        call::COMP_EVENT => {
            need(slot, cap::COMPONENT)?;
            crate::process::components::event(slot, a[0])
        }
        call::COMP_RESTORE => {
            need(slot, cap::COMPONENT)?;
            crate::process::components::restore(slot, a[0], a[1], a[2])
        }
        call::COMP_PUT => {
            need(slot, cap::PUBLISH)?;
            crate::process::components::put(slot, a[0])
        }
        call::COMP_READ => {
            need(slot, cap::PUBLISH)?;
            crate::process::components::read(slot, a[0])
        }
        call::BUILD => {
            need(slot, cap::PUBLISH)?;
            exchange_to(slot, a[0], Destination::Build)
        }
        call::BUILD_START => {
            need(slot, cap::PUBLISH)?;
            build_start(slot, a[0])
        }
        call::BUILD_POLL => {
            need(slot, cap::PUBLISH)?;
            build_poll(slot, a[0], a[1])
        }
        call::COMP_CONTROL => {
            need(slot, cap::PUBLISH)?;
            crate::process::components::control(slot, a[0])
        }
        call::EDIT_CALL => {
            need(slot, cap::EDIT)?;
            edit_call(slot, a[0], a[1], a[2], a[3])
        }
        _ => Err(Error::NotFound),
    }
}

/// EDIT_CALL: one open or command for the edit service, through the kernel's
/// journal. Only what an app may send gets through ([`editcore::ipc::for_app`]).
fn edit_call(
    slot: usize,
    msg: u64,
    msg_len: u64,
    reply: u64,
    reply_cap: u64,
) -> Result<u64, Error> {
    let len = usize::try_from(msg_len).map_err(|_| Error::Invalid)?;
    let room = usize::try_from(reply_cap).unwrap_or(usize::MAX);
    if len == 0 || len > kabi::MAX_MESSAGE || room < kabi::MAX_MESSAGE {
        return Err(Error::Invalid);
    }
    let mut message = alloc::vec![0u8; len];
    uaccess::copy_from_user(slot, msg, &mut message)?;
    editcore::ipc::for_app(&message).map_err(|_| Error::Invalid)?;
    let answer = crate::edit_session::call_for_app(&message);
    uaccess::copy_to_user(slot, reply, answer.as_bytes())?;
    Ok(answer.len() as u64)
}

/// BUILD_START: hands a build to the background job.
fn build_start(slot: usize, request_ptr: u64) -> Result<u64, Error> {
    let (_, request, timeout, response_cap) = read_http(slot, request_ptr)?;
    crate::build_job::start(request, timeout, response_cap).map(|()| 0)
}

/// BUILD_POLL: the job's reply, copied outside the job's lock. A process being
/// killed collects nothing, and a reply the copy fails on stays for the next poll.
fn build_poll(slot: usize, buf: u64, cap: u64) -> Result<u64, Error> {
    if crate::process::killed(slot) {
        return Err(Error::Dead);
    }
    let cap = usize::try_from(cap).unwrap_or(usize::MAX);
    let (len, reply) = crate::build_job::take(cap)?;
    if let Some(reply) = reply {
        let copied = uaccess::copy_to_user(slot, buf, &reply);
        let error = match copied {
            Err(error) => Some(error),
            Ok(()) if crate::process::killed(slot) => Some(Error::Dead),
            Ok(()) => None,
        };
        if let Some(error) = error {
            crate::build_job::put_back(reply);
            return Err(error);
        }
    }
    Ok(len as u64)
}
