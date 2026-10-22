//! Calls every process may make: exit, output, time, sleep, its own memory and
//! threads, the metrics snapshot and the model map.

use super::need;
use crate::process::uaccess;
use crate::{hpet, log, quiet, threads};
use kabi::{Error, MAX_ARGS, call, cap};

/// Most bytes one `write` prints.
const MAX_WRITE: usize = 4096;
const MAX_SLEEP_MS: u64 = 3_600_000;

/// The calls this area serves.
const CALLS: &[u64] = &[
    call::EXIT,
    call::WRITE,
    call::SLEEP,
    call::YIELD,
    call::BRK,
    call::TIME,
    call::ARGS,
    call::METRICS,
    call::INTERRUPTED,
    call::MODEL_MAP,
    call::THREAD_SPAWN,
    call::THREAD_EXIT,
    call::FUTEX_WAIT,
    call::FUTEX_WAKE,
];

/// One of [`CALLS`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, a: &[u64; 6]) -> Option<Result<u64, Error>> {
    CALLS.contains(&number).then(|| run(slot, number, a))
}

fn run(slot: usize, number: u64, a: &[u64; 6]) -> Result<u64, Error> {
    match number {
        call::EXIT => crate::process::exit_with(slot, a[0] & 0xFFFF_FFFF),
        call::WRITE => write(slot, a[0], a[1], a[2]),
        call::SLEEP => sleep(slot, a[0]),
        call::YIELD => {
            threads::yield_now();
            Ok(0)
        }
        call::BRK => crate::process::grow_heap(slot, a[0]),
        call::TIME => Ok(hpet::nanos()),
        call::ARGS => {
            let process = crate::process::identity(slot);
            let args = process.args();
            let len = args.len().min(usize::try_from(a[1]).unwrap_or(MAX_ARGS));
            uaccess::copy_to_user(slot, a[0], &args[..len])?;
            Ok(args.len() as u64)
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
        call::INTERRUPTED => Ok(u64::from(crate::process::take_interrupt(slot))),
        call::MODEL_MAP => {
            need(slot, cap::MODEL)?;
            crate::model::map(slot)
        }
        call::THREAD_SPAWN => {
            need(slot, cap::THREADS)?;
            crate::process::workers::spawn(slot, a[0], a[1], a[2], a[3])
        }
        call::THREAD_EXIT => crate::process::workers::exit_worker(),
        call::FUTEX_WAIT => crate::process::futex::wait(slot, a[0], a[1], a[2]),
        call::FUTEX_WAKE => crate::process::futex::wake(slot, a[0], a[1]),
        _ => Err(Error::NotFound),
    }
}

/// Prints the process's text, without control characters, one prefixed line at a
/// time. Lines start with `[name] `, which no kernel line does, so a program can
/// never imitate kernel output. With [`call::WRITE_LOG_ONLY`] they go to the log
/// only, not the screen.
fn write(slot: usize, ptr: u64, len: u64, flags: u64) -> Result<u64, Error> {
    if flags & !call::WRITE_LOG_ONLY != 0 {
        return Err(Error::Invalid);
    }
    let log_only = flags & call::WRITE_LOG_ONLY != 0;
    let len = usize::try_from(len).unwrap_or(usize::MAX).min(MAX_WRITE);
    let mut buf = [0u8; MAX_WRITE];
    uaccess::copy_from_user(slot, ptr, &mut buf[..len])?;
    let process = crate::process::identity(slot);
    let name = process.name.as_str();
    // Each piece is printed in one go (each print keeps interrupts off), and with
    // its `[name] ` fits one row, so ring-3 text never starts a row of its own.
    let piece = crate::terminal::PIECE_CHARS.saturating_sub(name.chars().count() + 3);
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
                .nth(piece.max(1))
                .map_or(rest.len(), |(at, _)| at);
            if log_only {
                quiet!("[{name}] {}", &rest[..cut]);
            } else {
                log!("[{name}] {}", &rest[..cut]);
            }
            rest = &rest[cut..];
            if rest.is_empty() {
                break;
            }
        }
    }
    // After the text is out, so whoever waits on it sees it printed.
    crate::process::mark_wrote(slot);
    Ok(len as u64)
}

fn sleep(slot: usize, ms: u64) -> Result<u64, Error> {
    let deadline = hpet::nanos().saturating_add(ms.min(MAX_SLEEP_MS) * 1_000_000);
    while hpet::nanos() < deadline && !crate::process::killed(slot) {
        threads::sleep_until_unless(deadline, || crate::process::killed(slot));
    }
    Ok(0)
}
