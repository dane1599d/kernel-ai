//! The network: HTTP to the LLM server, and host names.

use super::call_error;
use crate::process::uaccess;
use crate::{ipc, llm, log, net};
use alloc::vec::Vec;
use kabi::handle::{Handle, Kind, Rights};
use kabi::{Error, HttpRequest, call};

const MAX_HTTP_REQUEST: usize = 64 * 1024;
const MAX_HTTP_TIMEOUT_MS: u64 = 300_000;

/// The calls this area serves.
const CALLS: &[u64] = &[
    call::HTTP,
    call::RESOLVE,
    call::RESOLVE_ON,
    call::NETDEST_NARROW,
    call::NET_CONNECT,
    call::NET_SEND,
    call::NET_RECV,
    call::NET_POLL,
    call::NET_CLOSE,
];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| run(slot, number, a))
}

fn run(slot: usize, number: u64, a: &[u64; 6]) -> Result<u64, Error> {
    match number {
        // The LLM's own `NetDest`, which NET's startup handle is: a handle
        // to any other destination does not reach the LLM (K's review of #755).
        call::HTTP => {
            crate::handle::need_object(
                slot,
                Kind::NetDest,
                Rights::CONNECT,
                kabi::handle::netdest::LLM,
            )?;
            exchange_to(slot, a[0], Destination::Llm)
        }
        // Retired with `cap::RESOLVE` (P5 task 1): a resolve goes on a handle.
        call::RESOLVE => Err(Error::Permission),
        call::RESOLVE_ON => {
            let object = dest_object(slot, a[0], Rights::RESOLVE)?;
            resolve(slot, object, a[1], a[2], a[3], a[4])
        }
        call::NETDEST_NARROW => narrow(slot, a[0], a[1], a[2], a[3]),
        call::NET_CONNECT => connect(slot, a[0], a[1], a[2], a[3]),
        call::NET_SEND => {
            let id = stream_object(slot, a[0], Rights::WRITE)?;
            let len = usize::try_from(a[2]).map_err(|_| Error::TooBig)?;
            if len > net::stream::MAX_CALL {
                return Err(Error::TooBig);
            }
            let mut data = alloc::vec![0u8; len];
            uaccess::copy_from_user(slot, a[1], &mut data)?;
            net::stream::send(owner(slot), id, &data).map(|n| n as u64)
        }
        call::NET_RECV => {
            let id = stream_object(slot, a[0], Rights::READ)?;
            let cap = usize::try_from(a[2])
                .unwrap_or(usize::MAX)
                .min(net::stream::MAX_CALL);
            let mut data = alloc::vec![0u8; cap];
            let n = net::stream::recv(owner(slot), id, &mut data)?;
            uaccess::copy_to_user(slot, a[1], &data[..n])?;
            Ok(n as u64)
        }
        call::NET_POLL => {
            let id = stream_object(slot, a[0], Rights::WAIT)?;
            let cancel = || crate::process::killed(slot) || crate::process::interrupt_pending(slot);
            net::stream::poll(owner(slot), id, a[1], a[2], cancel)
        }
        call::NET_CLOSE => {
            let handle = handle_arg(a[0])?;
            let id = stream_object(slot, a[0], Rights::empty())?;
            net::stream::close(owner(slot), id)?;
            crate::handle::with(slot, |h| h.table.close(handle))?.map_err(Error::from)?;
            Ok(0)
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
    /// The package mirror, `pkgrepo=` (PKG_FETCH).
    Pkgrepo,
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
        // Only the one request `pkg` builds: a GET of one range of one file.
        Destination::Pkgrepo if kabi::pkgrepo::check(&request).is_err() => {
            return Err(Error::Invalid);
        }
        Destination::Pkgrepo => crate::pkgrepo::target(),
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
fn owner(slot: usize) -> u64 {
    crate::process::identity(slot).pid
}

/// A handle argument: 32 bits, never 0.
fn handle_arg(raw: u64) -> Result<Handle, Error> {
    u32::try_from(raw)
        .ok()
        .filter(|&h| h != 0)
        .map(Handle)
        .ok_or(Error::BadHandle)
}

/// What a `Stream` handle allows: its own data, to wait on it, and to look.
/// Neither DUPLICATE nor TRANSFER: a stream stays with the process that made
/// it, which is what [`net::stream::on_exit`] closes.
const STREAM_RIGHTS: Rights = Rights::READ
    .union(Rights::WRITE)
    .union(Rights::WAIT)
    .union(Rights::INSPECT);

/// The stream `raw`, a `Stream` handle with `rights`, names.
fn stream_object(slot: usize, raw: u64, rights: Rights) -> Result<u64, Error> {
    let entry = crate::handle::entry(slot, handle_arg(raw)?)?;
    if entry.kind != Kind::Stream {
        return Err(Error::BadHandle);
    }
    if !entry.rights.contains(rights) {
        return Err(Error::Permission);
    }
    Ok(entry.object)
}

/// NET_CONNECT: a stream to `addr`:`port` (the address in network order in
/// the low 32 bits) on the `NetDest` `raw`, which needs CONNECT and must
/// allow it ([`net::dest::may_connect`]). `flags` may ask for a bulk stream.
fn connect(slot: usize, raw: u64, addr: u64, port: u64, flags: u64) -> Result<u64, Error> {
    let object = dest_object(slot, raw, Rights::CONNECT)?;
    let addr = u32::try_from(addr).map_err(|_| Error::Invalid)?;
    let addr = core::net::Ipv4Addr::from(addr.to_be_bytes());
    let port = u16::try_from(port)
        .ok()
        .filter(|&p| p != 0)
        .ok_or(Error::Invalid)?;
    if flags & !kabi::net::STREAM_BULK != 0 {
        return Err(Error::Invalid);
    }
    if !net::dest::may_connect(object, netdest::Protocol::Tcp, addr, port) {
        return Err(Error::Permission);
    }
    let bulk = flags & kabi::net::STREAM_BULK != 0;
    let id = net::stream::connect(owner(slot), addr, port, bulk)?;
    let entry = captable::Entry::new(Kind::Stream, STREAM_RIGHTS, id);
    match crate::handle::give(slot, alloc::vec![entry]).first() {
        Some(&handle) if handle != Handle::NONE => Ok(u64::from(handle.0)),
        _ => {
            let _ = net::stream::close(owner(slot), id);
            Err(Error::NoMemory)
        }
    }
}

/// The record `raw`, a `NetDest` handle with `rights`, names: Permission
/// for another kind, a fixed destination or a right it lacks.
fn dest_object(slot: usize, raw: u64, rights: Rights) -> Result<u64, Error> {
    let handle = u32::try_from(raw)
        .ok()
        .filter(|&h| h != 0)
        .ok_or(Error::BadHandle)?;
    let entry = crate::handle::entry(slot, Handle(handle))?;
    if entry.kind != Kind::NetDest
        || !entry.rights.contains(rights)
        || entry.object < net::dest::FIRST
    {
        return Err(Error::Permission);
    }
    Ok(entry.object)
}

/// NETDEST_NARROW: a new handle, with `rights` (within the old one's), to a
/// record that allows no more than the one `raw` names, read from the text at
/// `spec_ptr`. Permission if it would allow more.
fn narrow(slot: usize, raw: u64, spec_ptr: u64, spec_len: u64, rights: u64) -> Result<u64, Error> {
    let object = dest_object(slot, raw, Rights::DUPLICATE)?;
    let held = crate::handle::entry(slot, Handle(raw as u32))?.rights;
    let rights = u32::try_from(rights)
        .ok()
        .and_then(Rights::from_bits)
        .ok_or(Error::Invalid)?;
    if !held.contains(rights) {
        return Err(Error::Permission);
    }
    let len = usize::try_from(spec_len).map_err(|_| Error::TooBig)?;
    if len == 0 || len > netdest::MAX_TEXT {
        return Err(Error::Invalid);
    }
    let mut text = alloc::vec![0u8; len];
    uaccess::copy_from_user(slot, spec_ptr, &mut text)?;
    let text = core::str::from_utf8(&text).map_err(|_| Error::Invalid)?;
    let narrower = netdest::Dest::parse(text).map_err(|_| Error::Invalid)?;
    let live = |object| crate::handle::names(Kind::NetDest, object);
    let object = net::dest::narrow(object, narrower, live)?;
    let entry = captable::Entry::new(Kind::NetDest, rights, object);
    match crate::handle::give(slot, alloc::vec![entry]).first() {
        Some(&handle) if handle != Handle::NONE => Ok(u64::from(handle.0)),
        _ => Err(Error::NoMemory),
    }
}

/// RESOLVE_ON: `host`'s addresses, if it is under the record `object`'s
/// suffixes, which keeps them for its connects within their TTL.
fn resolve(
    slot: usize,
    object: u64,
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
    // A name the record does not name is never looked up, so nothing it
    // carries leaves in a query; the form checked is the form looked up.
    let host = netdest::normalize(host).ok_or(Error::Permission)?;
    if !net::dest::allows_name(object, &host) {
        return Err(Error::Permission);
    }
    let host = host.as_str();
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
    net::dest::resolved(object, &found.addresses, found.ttl_s);
    Ok(found.addresses.len() as u64)
}
