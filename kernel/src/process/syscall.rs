//! System-call dispatch. Every argument is untrusted: sizes are capped before any
//! allocation, times are clamped, pointers are only used through `uaccess`, and
//! capability bits are checked before the call does anything.

use super::{supervision, uaccess};
use crate::entry::TrapFrame;
use crate::{cpu, hpet, ipc, llm, log, net, threads};
use alloc::vec::Vec;
use kabi::{Error, HttpRequest, MAX_ARGS, call, cap};
use x86_64::instructions::interrupts;

/// Most bytes one `write` prints.
const MAX_WRITE: usize = 4096;
/// Longest piece of a line printed in one go (each print keeps interrupts off).
const MAX_PIECE: usize = 160;
const MAX_SLEEP_MS: u64 = 3_600_000;
const MAX_HTTP_REQUEST: usize = 64 * 1024;
const MAX_HTTP_TIMEOUT_MS: u64 = 300_000;
/// Longest a process may wait for another service.
const MAX_CALL_MS: u64 = 150_000;
/// A process serving a call must hear back from its own call this long before the
/// call it serves times out.
pub(super) const NESTED_MARGIN_NS: u64 = 1_000_000_000;
/// Longest a process waits for a service that is not up yet.
pub(super) const MAX_BIND_WAIT_NS: u64 = 5_000_000_000;
/// Fastest policy switches may follow each other.
pub(crate) const MIN_POLICY_INTERVAL_NS: u64 = 100_000_000;

/// Called by `entry::syscall_entry` with interrupts off.
pub extern "C" fn dispatch(frame: &mut TrapFrame) {
    cpu::clac();
    interrupts::enable();
    let Some(slot) = super::current_slot() else {
        // A kernel thread cannot execute SYSCALL; treat it as fatal.
        interrupts::disable();
        crate::faults::fatal(frame);
    };
    let args = [
        frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9,
    ];
    frame.rax = match handle(slot, frame.rax, args) {
        Ok(value) => value,
        Err(error) => error.to_raw(),
    };
    interrupts::disable();
    super::return_to_user(frame);
}

fn need(slot: usize, bits: u64) -> Result<(), Error> {
    if super::identity(slot).limits.caps & bits == bits {
        Ok(())
    } else {
        Err(Error::Permission)
    }
}

fn handle(slot: usize, number: u64, a: [u64; 6]) -> Result<u64, Error> {
    if super::workers::is_worker(threads::current_tid()) && !super::workers::worker_may(number) {
        return Err(Error::Permission);
    }
    match number {
        call::EXIT => super::exit_with(slot, a[0] & 0xFFFF_FFFF),
        call::WRITE => write(slot, a[0], a[1]),
        call::SLEEP => sleep(slot, a[0]),
        call::YIELD => {
            threads::yield_now();
            Ok(0)
        }
        call::BRK => super::grow_heap(slot, a[0]),
        call::TIME => Ok(hpet::nanos()),
        call::ARGS => {
            let process = super::identity(slot);
            let args = process.args();
            let len = args.len().min(usize::try_from(a[1]).unwrap_or(MAX_ARGS));
            uaccess::copy_to_user(slot, a[0], &args[..len])?;
            Ok(args.len() as u64)
        }
        call::SPAWN => {
            need(slot, cap::SUPERVISE)?;
            supervision::spawn_service(slot, a[0])
        }
        call::WAIT_CHILD => {
            need(slot, cap::SUPERVISE)?;
            supervision::wait_child(slot, a[0], a[1])
        }
        call::KILL => {
            need(slot, cap::SUPERVISE)?;
            supervision::kill_service(a[0])
        }
        call::STATE_SAVE => {
            need(slot, cap::SUPERVISE)?;
            supervision::state_save(slot, a[0], a[1])
        }
        call::STATE_LOAD => {
            need(slot, cap::SUPERVISE)?;
            supervision::state_load(slot, a[0], a[1])
        }
        call::PLAN => {
            need(slot, cap::SUPERVISE)?;
            supervision::plan(slot, a[0], a[1])
        }
        call::LIST => {
            need(slot, cap::SUPERVISE)?;
            supervision::list(slot, a[0], a[1])
        }
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
        call::METRICS => {
            let metrics = crate::metrics::snapshot();
            // SAFETY: Metrics is plain old data.
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    (&raw const metrics).cast::<u8>(),
                    core::mem::size_of::<kabi::Metrics>(),
                )
            };
            uaccess::copy_to_user(slot, a[0], bytes)?;
            Ok(0)
        }
        call::POLICY => {
            need(slot, cap::POLICY)?;
            set_policy(a[0])
        }
        call::INTERRUPTED => Ok(u64::from(super::take_interrupt(slot))),
        call::COMP_FETCH => {
            need(slot, cap::COMPONENT)?;
            super::components::fetch(slot, a[0], a[1], a[2], a[3])
        }
        call::COMP_EVENT => {
            need(slot, cap::COMPONENT)?;
            super::components::event(slot, a[0])
        }
        call::COMP_RESTORE => {
            need(slot, cap::COMPONENT)?;
            super::components::restore(slot, a[0], a[1], a[2])
        }
        call::HTTP => {
            need(slot, cap::NET)?;
            exchange_to(slot, a[0], Destination::Llm)
        }
        call::COMP_PUT => {
            need(slot, cap::PUBLISH)?;
            super::components::put(slot, a[0])
        }
        call::COMP_READ => {
            need(slot, cap::PUBLISH)?;
            super::components::read(slot, a[0])
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
            super::components::control(slot, a[0])
        }
        call::MODEL_MAP => {
            need(slot, cap::MODEL)?;
            crate::model::map(slot)
        }
        call::THREAD_SPAWN => {
            need(slot, cap::THREADS)?;
            super::workers::spawn(slot, a[0], a[1], a[2], a[3])
        }
        call::THREAD_EXIT => super::workers::exit_worker(),
        call::FUTEX_WAIT => super::futex::wait(slot, a[0], a[1], a[2]),
        call::FUTEX_WAKE => super::futex::wake(slot, a[0], a[1]),
        _ => Err(Error::NotFound),
    }
}

/// Prints the process's text, without control characters, one prefixed line at a
/// time. Lines start with `[name] `, which no kernel line does, so a program can
/// never imitate kernel output.
fn write(slot: usize, ptr: u64, len: u64) -> Result<u64, Error> {
    let len = usize::try_from(len).unwrap_or(usize::MAX).min(MAX_WRITE);
    let mut buf = [0u8; MAX_WRITE];
    uaccess::copy_from_user(slot, ptr, &mut buf[..len])?;
    let process = super::identity(slot);
    let name = process.name.as_str();
    // A character cut by the size cap drops only that character, not the text.
    let text = match core::str::from_utf8(&buf[..len]) {
        Ok(text) => text,
        Err(error) if error.error_len().is_none() => {
            core::str::from_utf8(&buf[..error.valid_up_to()]).unwrap_or("")
        }
        Err(_) => "(not UTF-8)",
    };
    let clean = llmproto::text::printable(text, MAX_WRITE, 64);
    for line in clean.lines() {
        let mut rest = line;
        loop {
            let cut = rest
                .char_indices()
                .nth(MAX_PIECE)
                .map_or(rest.len(), |(at, _)| at);
            log!("[{name}] {}", &rest[..cut]);
            rest = &rest[cut..];
            if rest.is_empty() {
                break;
            }
        }
    }
    Ok(len as u64)
}

fn sleep(slot: usize, ms: u64) -> Result<u64, Error> {
    let deadline = hpet::nanos().saturating_add(ms.min(MAX_SLEEP_MS) * 1_000_000);
    while hpet::nanos() < deadline && !super::killed(slot) {
        threads::sleep_until_unless(deadline, || super::killed(slot));
    }
    Ok(0)
}

/// Where a ring-3 exchange goes: the kernel picks the address, never the caller.
enum Destination {
    /// The LLM server, `llm=` (HTTP).
    Llm,
    /// The build service, `build=` (BUILD).
    Build,
}

/// A ring-3 HTTP request: the struct, the request copied in (at most 64 KiB), the
/// timeout clamped to 300 s, and the reply's cap.
fn read_http(slot: usize, request_ptr: u64) -> Result<(HttpRequest, Vec<u8>, u64, usize), Error> {
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
/// closing the connection. The reply to a build (`POST /build`) is kept, so the
/// upload that follows can be checked against it.
fn exchange_to(slot: usize, request_ptr: u64, destination: Destination) -> Result<u64, Error> {
    let (call, request, timeout, response_cap) = read_http(slot, request_ptr)?;
    let target = match destination {
        Destination::Llm => llm::target().map(|(ip, port, _)| (ip, port)),
        Destination::Build => crate::build_service::target(),
    };
    let (ip, port) = target.ok_or(Error::NotFound)?;
    let builds = matches!(destination, Destination::Build) && request.starts_with(b"POST /build ");
    if builds {
        crate::build_service::count_build();
    }
    let response = net::exchange((ip, port), &request, timeout, response_cap, || {
        super::killed(slot)
    })
    .map_err(|error| {
        if !super::killed(slot) {
            log!(
                "net: request for {} failed: {error}",
                super::identity(slot).name.as_str()
            );
        }
        net::error_code(error)
    })?;
    drop(request);
    if super::killed(slot) {
        return Err(Error::Dead);
    }
    uaccess::copy_to_user(slot, call.response_ptr, &response)?;
    let len = response.len() as u64;
    if builds {
        crate::build_service::keep_build_reply(response);
    }
    Ok(len)
}

/// BUILD_START: hands a build to the background job.
fn build_start(slot: usize, request_ptr: u64) -> Result<u64, Error> {
    let (_, request, timeout, response_cap) = read_http(slot, request_ptr)?;
    crate::build_job::start(request, timeout, response_cap).map(|()| 0)
}

/// BUILD_POLL: the job's reply, copied outside the job's lock. A process being
/// killed collects nothing, and a reply the copy fails on stays for the next poll.
fn build_poll(slot: usize, buf: u64, cap: u64) -> Result<u64, Error> {
    if super::killed(slot) {
        return Err(Error::Dead);
    }
    let cap = usize::try_from(cap).unwrap_or(usize::MAX);
    let (len, reply) = crate::build_job::take(cap)?;
    if let Some(reply) = reply {
        let copied = uaccess::copy_to_user(slot, buf, &reply);
        let error = match copied {
            Err(error) => Some(error),
            Ok(()) if super::killed(slot) => Some(Error::Dead),
            Ok(()) => None,
        };
        if let Some(error) = error {
            crate::build_job::put_back(reply);
            return Err(error);
        }
    }
    Ok(len as u64)
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
    let me = super::identity(slot);
    if name == me.name.as_str() {
        return Err(Error::Invalid);
    }
    if super::programs::find(name).is_none() && !super::supervision::planned(name) {
        return Err(Error::NotFound);
    }
    if super::has_cap_by_name(name, cap::CALL) {
        return Err(Error::Permission);
    }
    // Only the kernel drives the publisher, whether or not it is running now.
    if name == super::supervision::PUBLISHER || super::has_cap_by_name(name, cap::PUBLISH) {
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
    let cancel = || super::killed(slot) || super::interrupt_pending(slot);
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

/// What a failed call made on a process's behalf returns to it.
pub(super) fn call_error(slot: usize, error: ipc::CallError) -> Error {
    match error {
        ipc::CallError::Cancelled if super::take_interrupt(slot) => Error::Interrupted,
        ipc::CallError::Cancelled | ipc::CallError::ServiceDied => Error::Dead,
        ipc::CallError::Unavailable => Error::Unavailable,
        ipc::CallError::TimedOut => Error::TimedOut,
        ipc::CallError::Busy => Error::Busy,
        ipc::CallError::NoMemory => Error::NoMemory,
        ipc::CallError::TooBig => Error::TooBig,
    }
}

/// Installs a scheduling policy on every CPU, unless it already is, or the last
/// switch was too recent (a loop of switches would disturb every CPU).
fn set_policy(id: u64) -> Result<u64, Error> {
    use core::sync::atomic::{AtomicU64, Ordering};
    static LAST_SWITCH_NS: AtomicU64 = AtomicU64::new(0);
    let (name, make): (&str, fn() -> alloc::boxed::Box<dyn sched::SchedPolicy>) = match id {
        kabi::policy::INTERACTIVE => ("interactive", || {
            alloc::boxed::Box::new(sched::Interactive::new())
        }),
        kabi::policy::ROUND_ROBIN => ("round-robin", || {
            alloc::boxed::Box::new(sched::RoundRobin::new())
        }),
        _ => return Err(Error::Invalid),
    };
    if threads::installed_policy_everywhere(name) {
        return Ok(0);
    }
    let now = hpet::nanos();
    let last = LAST_SWITCH_NS.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < MIN_POLICY_INTERVAL_NS {
        return Err(Error::Busy);
    }
    LAST_SWITCH_NS.store(now, Ordering::Relaxed);
    threads::set_policy_all(make);
    // A script that switches and then reads the policy must see the switch.
    crate::metrics::invalidate();
    log!("sched: policy switched to {name} by a ring-3 request");
    Ok(0)
}
