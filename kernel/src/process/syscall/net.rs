//! The network: HTTP to the LLM server, and host names.

use super::{call_error, need};
use crate::process::uaccess;
use crate::{ipc, llm, log, net};
use alloc::vec::Vec;
use kabi::{Error, HttpRequest, call, cap};

const MAX_HTTP_REQUEST: usize = 64 * 1024;
const MAX_HTTP_TIMEOUT_MS: u64 = 300_000;

/// The calls this area serves.
const CALLS: &[u64] = &[call::HTTP, call::RESOLVE];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| run(slot, number, a))
}

fn run(slot: usize, number: u64, a: &[u64; 6]) -> Result<u64, Error> {
    match number {
        call::HTTP => {
            need(slot, cap::NET)?;
            exchange_to(slot, a[0], Destination::Llm)
        }
        call::RESOLVE => {
            need(slot, cap::RESOLVE)?;
            resolve(slot, a[0], a[1], a[2], a[3])
        }
        _ => Err(Error::NotFound),
    }
}

/// Where a ring-3 exchange goes: the kernel picks the address, never the caller.
pub(super) enum Destination {
    /// The LLM server, `llm=` (HTTP).
    Llm,
    /// The build service, `build=` (BUILD).
    Build,
}

/// A ring-3 HTTP request: the struct, the request copied in (at most 64 KiB), the
/// timeout clamped to 300 s, and the reply's cap.
pub(super) fn read_http(
    slot: usize,
    request_ptr: u64,
) -> Result<(HttpRequest, Vec<u8>, u64, usize), Error> {
    let call: HttpRequest = uaccess::read_struct(slot, request_ptr)?;
    let request_len = usize::try_from(call.request_len).map_err(|_| Error::TooBig)?;
    if request_len > MAX_HTTP_REQUEST {
        return Err(Error::TooBig);
    }
    let response_cap = usize::try_from(call.response_cap)
        .unwrap_or(usize::MAX)
        .min(llmproto::http::MAX_RESPONSE);
    let mut request = Vec::new();
    request
        .try_reserve_exact(request_len)
        .map_err(|_| Error::NoMemory)?;
    request.resize(request_len, 0);
    uaccess::copy_from_user(slot, call.request_ptr, &mut request)?;
    let timeout = call.timeout_ms.clamp(1, MAX_HTTP_TIMEOUT_MS);
    Ok((call, request, timeout, response_cap))
}

/// Sends the request to the destination and returns what it answered up to
/// closing the connection. The build service takes only git syncs this way;
/// builds go through BUILD_START.
pub(super) fn exchange_to(
    slot: usize,
    request_ptr: u64,
    destination: Destination,
) -> Result<u64, Error> {
    let (call, request, timeout, response_cap) = read_http(slot, request_ptr)?;
    let target = match destination {
        Destination::Llm => llm::target().map(|(ip, port, _)| (ip, port)),
        Destination::Build if !editcore::wire::is_sync_request(&request) => {
            return Err(Error::Invalid);
        }
        Destination::Build => crate::build_service::target(),
    };
    let (ip, port) = target.ok_or(Error::NotFound)?;
    let response = net::exchange((ip, port), &request, timeout, response_cap, || {
        crate::process::killed(slot)
    })
    .map_err(|error| {
        if !crate::process::killed(slot) {
            log!(
                "net: request for {} failed: {error}",
                crate::process::identity(slot).name.as_str()
            );
        }
        net::error_code(error)
    })?;
    drop(request);
    if crate::process::killed(slot) {
        return Err(Error::Dead);
    }
    uaccess::copy_to_user(slot, call.response_ptr, &response)?;
    Ok(response.len() as u64)
}

/// RESOLVE: the host name copied in (at most `kabi::net::MAX_HOST` bytes), looked up
/// by the kernel's resolver, and its addresses copied out.
fn resolve(
    slot: usize,
    host_ptr: u64,
    host_len: u64,
    out: u64,
    out_cap: u64,
) -> Result<u64, Error> {
    let host_len = usize::try_from(host_len).map_err(|_| Error::Invalid)?;
    if host_len == 0 || host_len > kabi::net::MAX_HOST + 1 {
        return Err(Error::Invalid);
    }
    let mut host = [0u8; kabi::net::MAX_HOST + 1];
    uaccess::copy_from_user(slot, host_ptr, &mut host[..host_len])?;
    let host = core::str::from_utf8(&host[..host_len]).map_err(|_| Error::Invalid)?;
    let cancel = || crate::process::killed(slot) || crate::process::interrupt_pending(slot);
    let found =
        net::resolve::resolve(host, kabi::net::RESOLVE_TIMEOUT_MS, cancel).map_err(|error| {
            match error {
                "net: cancelled" => call_error(slot, ipc::CallError::Cancelled),
                other => net::error_code(other),
            }
        })?;
    let cap = usize::try_from(out_cap).unwrap_or(usize::MAX);
    let bytes: Vec<u8> = found
        .addresses
        .iter()
        .take(cap)
        .flat_map(|ip| ip.octets())
        .collect();
    uaccess::copy_to_user(slot, out, &bytes)?;
    Ok(found.addresses.len() as u64)
}
