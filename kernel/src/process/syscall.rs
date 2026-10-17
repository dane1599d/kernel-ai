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
            ipc::recv(slot, a[0], a[1], a[2])
        }
        call::SVC_REPLY => {
            need(slot, cap::SERVE)?;
            ipc::reply(slot, a[0], a[1])
        }
        call::HTTP => {
            need(slot, cap::NET)?;
            http(slot, a[0])
        }
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
    let text = core::str::from_utf8(&buf[..len]).unwrap_or("(not UTF-8)");
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

/// Sends the request to the configured LLM server and returns what it answered up
/// to closing the connection. The destination is the kernel's, not the caller's.
fn http(slot: usize, request_ptr: u64) -> Result<u64, Error> {
    let call: HttpRequest = uaccess::read_struct(slot, request_ptr)?;
    let (ip, port, _) = llm::target().ok_or(Error::NotFound)?;
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
        match error {
            "net: request timed out" => Error::TimedOut,
            "net: out of memory" => Error::NoMemory,
            _ => Error::Network,
        }
    })?;
    drop(request);
    if super::killed(slot) {
        return Err(Error::Dead);
    }
    uaccess::copy_to_user(slot, call.response_ptr, &response)?;
    Ok(response.len() as u64)
}
