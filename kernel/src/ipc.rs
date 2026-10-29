//! Service endpoints: the kernel calls a ring-3 service by name, and a process
//! calls one through a route handle (`CALL_H`), and each waits for its reply.
//! A call from a process carries its route's badge and rights, and may move
//! handles each way ([`Extra`]); the system-call layer checks and moves them.
//!
//! A service binds the endpoint named after its program on its first `svc_recv`; the
//! name comes from the kernel's process table, never from the service, so one
//! program cannot answer for another. The server is told who called (0 = the
//! kernel). Requests and replies are copied into kernel buffers and each side copies
//! to or from its own memory on its own thread.
//!
//! A call moves Queued -> Taken -> Replied (or Failed if its server dies, or is
//! dropped by the caller on timeout or cancellation). Its state is an atomic, so the
//! sleeping side's wake-up condition reads no lock; wakers publish the state, release
//! the IPC lock, and only then call `wake`. When a server dies, calls it had taken
//! fail at once and queued ones wait for the next instance.

use crate::process::{self, MAX_PROCESSES, Name, uaccess};
use crate::{hpet, threads};
use alloc::vec::Vec;
use captable::Entry;
use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use kabi::{Error, MAX_MESSAGE};
use sched::Tid;
use spin::Mutex;
use x86_64::instructions::interrupts;

const MAX_ENDPOINTS: usize = MAX_PROCESSES;
/// Every process can have one call outstanding, plus the kernel's.
const MAX_CALLS: usize = MAX_PROCESSES + 2;
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
    /// The caller gave up (killed, or Ctrl-C).
    Cancelled,
    NoMemory,
    TooBig,
}

/// What a call from a process carries besides its bytes (ADR 0001 D1, D6).
#[derive(Debug, Default)]
pub struct Extra {
    /// The badge and rights of the route it came through.
    pub badge: u64,
    pub rights: u32,
    /// Handles moved with the request, already out of the caller's table.
    pub handles: Vec<Entry>,
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
    /// The calling process, or 0 for the kernel.
    caller_pid: u64,
    deadline_ns: u64,
    taker: u64,
    extra: Extra,
    /// Handles moved with the reply, already out of the server's table.
    reply_handles: Vec<Entry>,
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
/// Each call slot's call id, so a server's compute allowance ends with its call.
static CALL_ID: [AtomicU64; MAX_CALLS] = [const { AtomicU64::new(0) }; MAX_CALLS];
static PENDING: [AtomicU32; MAX_ENDPOINTS] = [const { AtomicU32::new(0) }; MAX_ENDPOINTS];
static BOUND: [AtomicU64; MAX_ENDPOINTS] = [const { AtomicU64::new(0) }; MAX_ENDPOINTS];
/// Replies each endpoint delivered to the kernel and to processes.
static REPLIES_TO_KERNEL: [AtomicU64; MAX_ENDPOINTS] = [const { AtomicU64::new(0) }; MAX_ENDPOINTS];
static REPLIES_TO_PROCESSES: [AtomicU64; MAX_ENDPOINTS] =
    [const { AtomicU64::new(0) }; MAX_ENDPOINTS];
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

/// Wakes the server of process `pid` if it waits in `svc_recv`, which then
/// returns `Woken` (an event box it reads got an event).
pub fn wake_server(pid: u64) {
    let waiting = with_ipc(|ipc| {
        ipc.endpoints
            .iter()
            .flatten()
            .find(|e| e.server_pid == pid)
            .and_then(|e| e.waiting)
    });
    wake(waiting);
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

/// For a server waiting in another call (the supervisor in `wait_child`): binds
/// its endpoint as `svc_recv` does and marks `tid` waiting on it, so the next
/// request wakes it; returns the endpoint for [`pending`] and [`unwatch`].
pub fn watch(slot: usize, tid: Tid) -> Result<usize, Error> {
    let me = process::identity(slot);
    with_ipc(|ipc| {
        let endpoint = bind(ipc, &me)?;
        if let Some(entry) = ipc.endpoints[endpoint].as_mut() {
            entry.waiting = Some(tid);
        }
        Ok(endpoint)
    })
}

/// Whether a request waits on `endpoint`.
pub fn pending(endpoint: usize) -> bool {
    PENDING[endpoint].load(Ordering::SeqCst) > 0
}

/// Ends [`watch`]'s wait for `tid`.
pub fn unwatch(endpoint: usize, tid: Tid) {
    with_ipc(|ipc| {
        if let Some(entry) = ipc.endpoints[endpoint].as_mut()
            && entry.waiting == Some(tid)
        {
            entry.waiting = None;
        }
    });
}

/// The endpoint named after `me`, made if there is none yet, with `me` as its
/// server; Busy if another process serves it.
fn bind(ipc: &mut Ipc, me: &process::Process) -> Result<usize, Error> {
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
    Ok(endpoint)
}

/// Replies `name` has delivered so far: (to the kernel, to processes).
pub fn replies(name: &str) -> (u64, u64) {
    with_ipc(|ipc| find(ipc, name)).map_or((0, 0), |e| {
        (
            REPLIES_TO_KERNEL[e].load(Ordering::SeqCst),
            REPLIES_TO_PROCESSES[e].load(Ordering::SeqCst),
        )
    })
}

/// Deadline of the call the process `pid` is serving right now, if any, so a call
/// it makes in turn can be made to end first.
pub fn serving_deadline(pid: u64) -> Option<u64> {
    with_ipc(|ipc| {
        let endpoint = ipc
            .endpoints
            .iter()
            .flatten()
            .find(|e| e.server_pid == pid)?;
        let call = ipc.calls[endpoint.current_call?].as_ref()?;
        Some(call.deadline_ns)
    })
}

/// Waits until a server has bound `name`, `deadline_ns` passes, or `cancel()`.
pub fn wait_bound(
    name: &str,
    deadline_ns: u64,
    cancel: impl Fn() -> bool,
) -> Result<(), CallError> {
    while server_of(name).is_none() {
        if cancel() {
            return Err(CallError::Cancelled);
        }
        if hpet::nanos() >= deadline_ns {
            return Err(CallError::Unavailable);
        }
        threads::sleep_ms(BIND_POLL_MS);
    }
    Ok(())
}

/// A call that was sent and not yet collected with [`finish`].
pub struct Pending {
    slot: usize,
}

impl Pending {
    /// The reply (or a failure) is in.
    pub fn done(&self) -> bool {
        matches!(
            CALL_STATE[self.slot].load(Ordering::SeqCst),
            REPLIED | FAILED
        )
    }
}

/// Queues `request` for the bound service `name`.
pub fn submit(
    name: &str,
    request: &[u8],
    caller_pid: u64,
    deadline_ns: u64,
) -> Result<Pending, CallError> {
    submit_with(name, request, caller_pid, deadline_ns, Extra::default())
}

/// [`submit`], with what a handle call carries.
fn submit_with(
    name: &str,
    request: &[u8],
    caller_pid: u64,
    deadline_ns: u64,
    extra: Extra,
) -> Result<Pending, CallError> {
    if request.len() > MAX_MESSAGE {
        return Err(CallError::TooBig);
    }
    let mut copy = Vec::new();
    copy.try_reserve_exact(request.len())
        .map_err(|_| CallError::NoMemory)?;
    copy.extend_from_slice(request);
    let me = threads::current_tid();
    let (slot, server) = with_ipc(|ipc| {
        let endpoint = find(ipc, name)
            .filter(|&e| BOUND[e].load(Ordering::SeqCst) != 0)
            .ok_or(CallError::Unavailable)?;
        let slot = ipc
            .calls
            .iter()
            .position(Option::is_none)
            .ok_or(CallError::Busy)?;
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        CALL_ID[slot].store(id, Ordering::SeqCst);
        ipc.calls[slot] = Some(Call {
            id,
            endpoint,
            request: copy,
            reply: Vec::new(),
            caller: me,
            caller_pid,
            deadline_ns,
            taker: 0,
            extra,
            reply_handles: Vec::new(),
        });
        CALL_STATE[slot].store(QUEUED, Ordering::SeqCst);
        PENDING[endpoint].fetch_add(1, Ordering::SeqCst);
        let server = ipc.endpoints[endpoint]
            .as_mut()
            .and_then(|e| e.waiting.take());
        Ok((slot, server))
    })?;
    wake(server);
    Ok(Pending { slot })
}

/// Collects a call: its reply and the pid that answered, or why there is none. A
/// call still running is abandoned (its late reply is dropped) and reported as
/// timed out.
pub fn finish(pending: Pending) -> Result<(Vec<u8>, u64), CallError> {
    finish_with(pending).map(|(reply, taker, _)| (reply, taker))
}

/// [`finish`], with the handles the reply moved.
fn finish_with(pending: Pending) -> Result<(Vec<u8>, u64, Vec<Entry>), CallError> {
    let slot = pending.slot;
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
        (Some(call), REPLIED) => {
            let counter = if call.caller_pid == 0 {
                &REPLIES_TO_KERNEL[call.endpoint]
            } else {
                &REPLIES_TO_PROCESSES[call.endpoint]
            };
            counter.fetch_add(1, Ordering::SeqCst);
            Ok((call.reply, call.taker, call.reply_handles))
        }
        (_, FAILED) => Err(CallError::ServiceDied),
        _ => Err(CallError::TimedOut),
    }
}

/// Sends `request` to `name` on behalf of `caller_pid` and waits for the reply until
/// `deadline_ns`, or until `cancel()` (checked whenever the caller wakes).
pub fn call_as(
    name: &str,
    request: &[u8],
    caller_pid: u64,
    deadline_ns: u64,
    cancel: impl Fn() -> bool,
) -> Result<Vec<u8>, CallError> {
    call_as_from(name, request, caller_pid, deadline_ns, cancel).map(|(reply, _)| reply)
}

/// [`call_as`], with the pid that answered.
fn call_as_from(
    name: &str,
    request: &[u8],
    caller_pid: u64,
    deadline_ns: u64,
    cancel: impl Fn() -> bool,
) -> Result<(Vec<u8>, u64), CallError> {
    call_with(
        name,
        request,
        caller_pid,
        deadline_ns,
        cancel,
        Extra::default(),
    )
    .map(|(reply, taker, _)| (reply, taker))
}

/// A handle call (`CALL_H`): [`call_as`] carrying `extra`; the reply, the pid
/// that answered and the handles the reply moved.
pub fn call_with(
    name: &str,
    request: &[u8],
    caller_pid: u64,
    deadline_ns: u64,
    cancel: impl Fn() -> bool,
    extra: Extra,
) -> Result<(Vec<u8>, u64, Vec<Entry>), CallError> {
    let pending = submit_with(name, request, caller_pid, deadline_ns, extra)?;
    while !pending.done() && !cancel() && hpet::nanos() < deadline_ns {
        threads::sleep_until_unless(deadline_ns, || pending.done() || cancel());
    }
    let cancelled = !pending.done() && cancel();
    match finish_with(pending) {
        Err(CallError::TimedOut) if cancelled => Err(CallError::Cancelled),
        other => other,
    }
}

/// The kernel calls the service `name`, waiting up to `timeout_ms` in total
/// (including waiting for the service to come up).
pub fn call(name: &str, request: &[u8], timeout_ms: u64) -> Result<Vec<u8>, CallError> {
    call_from(name, request, timeout_ms).map(|(reply, _)| reply)
}

/// [`call`], with the pid that answered: a service restarted while the call was
/// queued answers as its new process.
pub fn call_from(name: &str, request: &[u8], timeout_ms: u64) -> Result<(Vec<u8>, u64), CallError> {
    let deadline = hpet::nanos().saturating_add(timeout_ms.saturating_mul(1_000_000));
    wait_bound(name, deadline, || false)?;
    call_as_from(name, request, 0, deadline, || false)
}

/// The `svc_recv` system call: waits for the next request to this process's service.
/// Writes the caller's pid (0 for the kernel) to `caller_out` unless it is 0.
/// Handles the request moved are dropped: a server that takes handles uses
/// `RECV_H` ([`recv_with`]).
pub fn recv(
    slot: usize,
    buf: u64,
    cap: u64,
    timeout_ms: u64,
    caller_out: u64,
) -> Result<u64, Error> {
    let (len, caller_pid, _) = recv_with(slot, buf, cap, timeout_ms)?;
    if caller_out != 0 {
        uaccess::copy_to_user(slot, caller_out, &caller_pid.to_le_bytes())?;
    }
    Ok(len)
}

/// Waits for the next request to this process's service, as `svc_recv` does; its
/// length, the caller's pid and what the call carried.
pub fn recv_with(
    slot: usize,
    buf: u64,
    cap: u64,
    timeout_ms: u64,
) -> Result<(u64, u64, Extra), Error> {
    let me = process::identity(slot);
    let tid = threads::current_tid();
    let wait_ms = if timeout_ms == 0 {
        MAX_WAIT_MS
    } else {
        timeout_ms.min(MAX_WAIT_MS)
    };
    let deadline = hpet::nanos().saturating_add(wait_ms * 1_000_000);
    loop {
        let (endpoint, taken, failed, opened, extra) = with_ipc(|ipc| {
            let endpoint = bind(ipc, &me)?;
            // The next queued call, copied before any state changes, so running out
            // of memory here leaves every call as it was.
            let next = (0..MAX_CALLS)
                .filter(|&c| CALL_STATE[c].load(Ordering::SeqCst) == QUEUED)
                .filter(|&c| {
                    ipc.calls[c]
                        .as_ref()
                        .is_some_and(|call| call.endpoint == endpoint)
                })
                .min_by_key(|&c| ipc.calls[c].as_ref().map_or(u64::MAX, |call| call.id));
            let request = match next.and_then(|c| ipc.calls[c].as_ref()) {
                Some(call) => {
                    let mut request = Vec::new();
                    request
                        .try_reserve_exact(call.request.len())
                        .map_err(|_| Error::NoMemory)?;
                    request.extend_from_slice(&call.request);
                    Some((request, call.caller_pid, (call.id, call.deadline_ns)))
                }
                None => None,
            };
            // A call taken before and never answered fails now.
            let mut failed = None;
            if let Some(old) = ipc.endpoints[endpoint]
                .as_mut()
                .and_then(|entry| entry.current_call.take())
                && CALL_STATE[old]
                    .compare_exchange(TAKEN, FAILED, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                failed = ipc.calls[old].as_ref().map(|c| c.caller);
            }
            let mut opened = None;
            let mut extra = Extra::default();
            match (next, &request) {
                (Some(c), Some((_, _, (id, deadline)))) => {
                    opened = Some((c, *id, *deadline));
                    if let Some(call) = ipc.calls[c].as_mut() {
                        call.taker = me.pid;
                        extra = core::mem::take(&mut call.extra);
                    }
                    CALL_STATE[c].store(TAKEN, Ordering::SeqCst);
                    PENDING[endpoint].fetch_sub(1, Ordering::SeqCst);
                    if let Some(entry) = ipc.endpoints[endpoint].as_mut() {
                        entry.current_call = Some(c);
                    }
                }
                _ => {
                    if let Some(entry) = ipc.endpoints[endpoint].as_mut() {
                        entry.waiting = Some(tid);
                    }
                }
            }
            Ok::<_, Error>((endpoint, request, failed, opened, extra))
        })?;
        wake(failed);
        if let Some((request, caller_pid, _)) = taken {
            let len = request
                .len()
                .min(usize::try_from(cap).unwrap_or(usize::MAX));
            uaccess::copy_to_user(slot, buf, &request[..len])?;
            // Only a request the server holds opens its compute allowance.
            process::burst::serving(slot, opened);
            return Ok((request.len() as u64, caller_pid, extra));
        }
        // No request: an event box this process reads may have woken it.
        if crate::event_box::take_wake(me.pid) {
            with_ipc(|ipc| {
                if let Some(entry) = ipc.endpoints[endpoint].as_mut() {
                    entry.waiting = None;
                }
            });
            return Err(Error::Woken);
        }
        let ready = || {
            PENDING[endpoint].load(Ordering::SeqCst) > 0
                || process::killed(slot)
                || crate::event_box::woken(me.pid)
        };
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
    reply_with(slot, ptr, len, Vec::new())
}

/// The caller of the call this process took and has not answered: its pid (0 for
/// the kernel), for checking the handles a reply would move to it.
pub fn current_caller(slot: usize) -> Result<u64, Error> {
    let me = process::identity(slot);
    with_ipc(|ipc| {
        let endpoint = find(ipc, me.name.as_str()).ok_or(Error::NotFound)?;
        let entry = ipc.endpoints[endpoint].as_ref().ok_or(Error::NotFound)?;
        let c = entry.current_call.ok_or(Error::NotFound)?;
        let call = ipc.calls[c].as_ref().ok_or(Error::NotFound)?;
        match call.taker == me.pid && CALL_STATE[c].load(Ordering::SeqCst) == TAKEN {
            true => Ok(call.caller_pid),
            false => Err(Error::NotFound),
        }
    })
}

/// [`reply`], moving `handles` (already out of the server's table) to the caller.
pub fn reply_with(slot: usize, ptr: u64, len: u64, handles: Vec<Entry>) -> Result<u64, Error> {
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
        call.reply_handles = handles;
        CALL_STATE[c].store(REPLIED, Ordering::SeqCst);
        Ok(call.caller)
    })?;
    process::burst::serving(slot, None);
    wake(Some(caller));
    Ok(len as u64)
}

/// Whether call slot `c` still holds call `id`, taken by its server and not yet
/// answered or dropped. Lock-free, for the burst accounting on the tick.
pub fn call_open(c: usize, id: u64) -> bool {
    c < MAX_CALLS
        && CALL_STATE[c].load(Ordering::SeqCst) == TAKEN
        && CALL_ID[c].load(Ordering::SeqCst) == id
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
