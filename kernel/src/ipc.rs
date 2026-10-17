//! Service endpoints: the kernel (the terminal, for now) calls a ring-3 service by
//! name and waits for its reply.
//!
//! A service binds the endpoint named after its program on its first `svc_recv`; the
//! name comes from the kernel's process table, never from the service, so one
//! program cannot answer for another. Requests and replies are copied into kernel
//! buffers and each side copies to or from its own memory on its own thread.
//!
//! A call moves Queued -> Taken -> Replied (or Failed if its server dies, or is
//! dropped by the caller on timeout). Its state is an atomic, so the sleeping side's
//! wake-up condition reads no lock; wakers publish the state, release the IPC lock,
//! and only then call `wake`. When a server dies, calls it had taken fail at once and
//! queued ones wait for the next instance.

use crate::process::{self, Name, uaccess};
use crate::{hpet, threads};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use kabi::{Error, MAX_MESSAGE};
use sched::Tid;
use spin::Mutex;
use x86_64::instructions::interrupts;

const MAX_ENDPOINTS: usize = 4;
const MAX_CALLS: usize = 4;
/// Longest a server waits in one `svc_recv`.
const MAX_WAIT_MS: u64 = 60_000;
/// How often a caller looks for a service that is not up yet.
const BIND_POLL_MS: u64 = 10;

const FREE: u8 = 0;
const QUEUED: u8 = 1;
const TAKEN: u8 = 2;
const REPLIED: u8 = 3;
const FAILED: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallError {
    /// No such service came up in time.
    Unavailable,
    /// Too many calls in flight.
    Busy,
    TimedOut,
    /// The service died while handling the call.
    ServiceDied,
    NoMemory,
    TooBig,
}

struct Endpoint {
    name: Name,
    server_pid: u64,
    /// The server, while it waits in `svc_recv`.
    waiting: Option<Tid>,
    current_call: Option<usize>,
}

struct Call {
    id: u64,
    endpoint: usize,
    request: Vec<u8>,
    reply: Vec<u8>,
    caller: Tid,
    taker: u64,
}

struct Ipc {
    endpoints: [Option<Endpoint>; MAX_ENDPOINTS],
    calls: [Option<Call>; MAX_CALLS],
}

static IPC: Mutex<Ipc> = Mutex::new(Ipc {
    endpoints: [const { None }; MAX_ENDPOINTS],
    calls: [const { None }; MAX_CALLS],
});
static CALL_STATE: [AtomicU8; MAX_CALLS] = [const { AtomicU8::new(FREE) }; MAX_CALLS];
static PENDING: [AtomicU32; MAX_ENDPOINTS] = [const { AtomicU32::new(0) }; MAX_ENDPOINTS];
static BOUND: [AtomicU64; MAX_ENDPOINTS] = [const { AtomicU64::new(0) }; MAX_ENDPOINTS];
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn with_ipc<R>(f: impl FnOnce(&mut Ipc) -> R) -> R {
    threads::preempt_disable();
    let result = f(&mut IPC.lock());
    threads::preempt_enable();
    result
}

fn wake(tid: Option<Tid>) {
    if let Some(tid) = tid {
        interrupts::without_interrupts(|| threads::wake(tid));
    }
}

fn find(ipc: &Ipc, name: &str) -> Option<usize> {
    ipc.endpoints
        .iter()
        .position(|e| e.as_ref().is_some_and(|e| e.name.as_str() == name))
}

/// The pid serving `name`, if any.
pub fn server_of(name: &str) -> Option<u64> {
    let pid = with_ipc(|ipc| find(ipc, name).map(|e| BOUND[e].load(Ordering::SeqCst)));
    pid.filter(|&pid| pid != 0)
}

/// Sends `request` to the service `name` and waits for its reply, for at most
/// `timeout_ms` in total (including waiting for the service to come up).
pub fn call(name: &str, request: &[u8], timeout_ms: u64) -> Result<Vec<u8>, CallError> {
    if request.len() > MAX_MESSAGE {
        return Err(CallError::TooBig);
    }
    let deadline = hpet::nanos().saturating_add(timeout_ms.saturating_mul(1_000_000));
    while server_of(name).is_none() {
        if hpet::nanos() >= deadline {
            return Err(CallError::Unavailable);
        }
        threads::sleep_ms(BIND_POLL_MS);
    }
    let mut copy = Vec::new();
    copy.try_reserve_exact(request.len())
        .map_err(|_| CallError::NoMemory)?;
    copy.extend_from_slice(request);
    let me = threads::current_tid();
    let (slot, server) = with_ipc(|ipc| {
        let endpoint = find(ipc, name).ok_or(CallError::Unavailable)?;
        let slot = ipc
            .calls
            .iter()
            .position(Option::is_none)
            .ok_or(CallError::Busy)?;
        ipc.calls[slot] = Some(Call {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            endpoint,
            request: copy,
            reply: Vec::new(),
            caller: me,
            taker: 0,
        });
        CALL_STATE[slot].store(QUEUED, Ordering::SeqCst);
        PENDING[endpoint].fetch_add(1, Ordering::SeqCst);
        let server = ipc.endpoints[endpoint]
            .as_mut()
            .and_then(|e| e.waiting.take());
        Ok((slot, server))
    })?;
    wake(server);
    let done = || matches!(CALL_STATE[slot].load(Ordering::SeqCst), REPLIED | FAILED);
    while !done() && hpet::nanos() < deadline {
        threads::sleep_until_unless(deadline, done);
    }
    let (call, state) = with_ipc(|ipc| {
        let call = ipc.calls[slot].take();
        let state = CALL_STATE[slot].swap(FREE, Ordering::SeqCst);
        if let Some(call) = &call {
            if state == QUEUED {
                PENDING[call.endpoint].fetch_sub(1, Ordering::SeqCst);
            }
            if let Some(endpoint) = ipc.endpoints[call.endpoint].as_mut()
                && endpoint.current_call == Some(slot)
            {
                endpoint.current_call = None;
            }
        }
        (call, state)
    });
    match (call, state) {
        (Some(call), REPLIED) => Ok(call.reply),
        (_, FAILED) => Err(CallError::ServiceDied),
        _ => Err(CallError::TimedOut),
    }
}

/// The `svc_recv` system call: waits for the next request to this process's service.
pub fn recv(slot: usize, buf: u64, cap: u64, timeout_ms: u64) -> Result<u64, Error> {
    let me = process::identity(slot);
    let tid = threads::current_tid();
    let wait_ms = if timeout_ms == 0 {
        MAX_WAIT_MS
    } else {
        timeout_ms.min(MAX_WAIT_MS)
    };
    let deadline = hpet::nanos().saturating_add(wait_ms * 1_000_000);
    loop {
        let (endpoint, taken, failed) = with_ipc(|ipc| {
            let endpoint = match find(ipc, me.name.as_str()) {
                Some(endpoint) => endpoint,
                None => {
                    let free = ipc
                        .endpoints
                        .iter()
                        .position(Option::is_none)
                        .ok_or(Error::Busy)?;
                    ipc.endpoints[free] = Some(Endpoint {
                        name: me.name,
                        server_pid: 0,
                        waiting: None,
                        current_call: None,
                    });
                    free
                }
            };
            let entry = ipc.endpoints[endpoint].as_mut().ok_or(Error::Busy)?;
            if entry.server_pid != 0 && entry.server_pid != me.pid {
                return Err(Error::Busy);
            }
            entry.server_pid = me.pid;
            BOUND[endpoint].store(me.pid, Ordering::SeqCst);
            // A call taken before and never answered fails now.
            let mut failed = None;
            if let Some(old) = entry.current_call.take()
                && CALL_STATE[old]
                    .compare_exchange(TAKEN, FAILED, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                failed = ipc.calls[old].as_ref().map(|c| c.caller);
            }
            let next = (0..MAX_CALLS)
                .filter(|&c| CALL_STATE[c].load(Ordering::SeqCst) == QUEUED)
                .filter(|&c| {
                    ipc.calls[c]
                        .as_ref()
                        .is_some_and(|call| call.endpoint == endpoint)
                })
                .min_by_key(|&c| ipc.calls[c].as_ref().map_or(u64::MAX, |call| call.id));
            let taken = match next {
                Some(c) => {
                    let call = ipc.calls[c].as_mut().ok_or(Error::Busy)?;
                    let mut request = Vec::new();
                    request
                        .try_reserve_exact(call.request.len())
                        .map_err(|_| Error::NoMemory)?;
                    request.extend_from_slice(&call.request);
                    call.taker = me.pid;
                    CALL_STATE[c].store(TAKEN, Ordering::SeqCst);
                    PENDING[endpoint].fetch_sub(1, Ordering::SeqCst);
                    if let Some(entry) = ipc.endpoints[endpoint].as_mut() {
                        entry.current_call = Some(c);
                    }
                    Some(request)
                }
                None => {
                    if let Some(entry) = ipc.endpoints[endpoint].as_mut() {
                        entry.waiting = Some(tid);
                    }
                    None
                }
            };
            Ok((endpoint, taken, failed))
        })?;
        wake(failed);
        if let Some(request) = taken {
            let len = request
                .len()
                .min(usize::try_from(cap).unwrap_or(usize::MAX));
            uaccess::copy_to_user(slot, buf, &request[..len])?;
            return Ok(request.len() as u64);
        }
        let ready = || PENDING[endpoint].load(Ordering::SeqCst) > 0 || process::killed(slot);
        if !ready() && hpet::nanos() < deadline {
            threads::sleep_until_unless(deadline, ready);
        }
        with_ipc(|ipc| {
            if let Some(entry) = ipc.endpoints[endpoint].as_mut() {
                entry.waiting = None;
            }
        });
        if process::killed(slot) {
            return Err(Error::Dead);
        }
        if PENDING[endpoint].load(Ordering::SeqCst) == 0 && hpet::nanos() >= deadline {
            return Err(Error::TimedOut);
        }
    }
}

/// The `svc_reply` system call: answers the call taken by the last `svc_recv`.
pub fn reply(slot: usize, ptr: u64, len: u64) -> Result<u64, Error> {
    let len = usize::try_from(len).map_err(|_| Error::TooBig)?;
    if len > MAX_MESSAGE {
        return Err(Error::TooBig);
    }
    let mut reply = Vec::new();
    reply.try_reserve_exact(len).map_err(|_| Error::NoMemory)?;
    reply.resize(len, 0);
    uaccess::copy_from_user(slot, ptr, &mut reply)?;
    let me = process::identity(slot);
    let caller = with_ipc(|ipc| {
        let endpoint = find(ipc, me.name.as_str()).ok_or(Error::NotFound)?;
        let entry = ipc.endpoints[endpoint].as_mut().ok_or(Error::NotFound)?;
        if entry.server_pid != me.pid {
            return Err(Error::NotFound);
        }
        let c = entry.current_call.take().ok_or(Error::NotFound)?;
        let call = ipc.calls[c].as_mut().ok_or(Error::NotFound)?;
        if call.taker != me.pid || CALL_STATE[c].load(Ordering::SeqCst) != TAKEN {
            return Err(Error::NotFound);
        }
        call.reply = reply;
        CALL_STATE[c].store(REPLIED, Ordering::SeqCst);
        Ok(call.caller)
    })?;
    wake(Some(caller));
    Ok(len as u64)
}

/// A process died: unbind its endpoints and fail the calls it had taken.
pub fn on_process_exit(pid: u64) {
    let mut callers = [None; MAX_CALLS];
    with_ipc(|ipc| {
        for (index, slot) in ipc.endpoints.iter_mut().enumerate() {
            let Some(endpoint) = slot.as_mut().filter(|e| e.server_pid == pid) else {
                continue;
            };
            endpoint.server_pid = 0;
            endpoint.waiting = None;
            BOUND[index].store(0, Ordering::SeqCst);
            endpoint.current_call = None;
        }
        for (c, call) in ipc.calls.iter().enumerate() {
            if let Some(call) = call.as_ref().filter(|call| call.taker == pid)
                && CALL_STATE[c]
                    .compare_exchange(TAKEN, FAILED, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                callers[c] = Some(call.caller);
            }
        }
    });
    for caller in callers {
        wake(caller);
    }
}
