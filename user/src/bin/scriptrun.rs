//! `scriptrun` (ADR 0001 D6): a script's own process. The agent core starts it
//! with JOB_SPAWN, holding no capability bits: only the run-tier handles moved in
//! under `request N`, so revoking that node leaves it nothing.
//!
//! The kernel's half (P1 task 14): `mode=probe` reads its Sched handle, checks
//! that a call needing a handle it was not given is refused, and writes
//! `ok <handles>` to the start of the `Shm` it was given.
//!
//! A-core's half: a `mode=script` claim reads the shell's request from its
//! `Shm`, checks the script again and runs it with only the handles moved in
//! (`shellcore::apart::serve`), and writes the answer back over the request.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::String;
use kabi::Error;
use kabi::handle::{HandleInfo, Kind};
use kabi::sched::SchedStats;
use llmproto::script::{Answer, AnswerError, ScriptRequest};
use shellcore::Host;
use user::{arg, args, println, sys};

user::entry!(main);

fn main() -> i32 {
    let args = args();
    match arg(&args, "mode") {
        Some("probe") => match probe() {
            Ok(()) => 0,
            Err(what) => {
                println!("scriptrun: FAILED: {what}");
                1
            }
        },
        Some("pooled") => pooled(),
        _ => {
            println!("scriptrun: no script engine here yet (mode=probe only)");
            2
        }
    }
}

/// A pooled child: takes each claim, runs it, and reports 0 or 1 with JOB_DONE,
/// which empties its table for the next one.
fn pooled() -> i32 {
    let mut buf = [0u8; 256];
    loop {
        let len = match sys::job_take(&mut buf) {
            Ok(len) => len.min(buf.len()),
            Err(Error::TimedOut) => continue,
            Err(error) => {
                println!("scriptrun: JOB_TAKE failed: {error:?}");
                return 1;
            }
        };
        let claim = core::str::from_utf8(&buf[..len]).unwrap_or("");
        let code = match arg(claim, "mode") {
            Some("probe") => match probe() {
                Ok(()) => 0,
                Err(what) => {
                    println!("scriptrun: FAILED: {what}");
                    1
                }
            },
            Some("script") => match script() {
                Ok(()) => 0,
                Err(what) => {
                    println!("scriptrun: FAILED: {what}");
                    1
                }
            },
            // A script that never finishes, for the kill test.
            Some("spin") => loop {
                core::hint::spin_loop();
            },
            _ => 2,
        };
        if let Err(error) = sys::job_done(code) {
            println!("scriptrun: JOB_DONE failed: {error:?}");
            return 1;
        }
    }
}

/// One script: the request at the start of the moved-in `Shm` (its length, then
/// its bytes), the answer written over it the same way.
fn script() -> Result<(), &'static str> {
    let shm = moved_in(Kind::Shm).ok_or("no Shm handle moved in")?;
    let address = sys::shm_map(shm, true).map_err(|_| "cannot map the request")?;
    // SAFETY: SHM_MAP mapped the shell's pages read-write at `address`; the shell
    // does not touch them until this claim is done. Its first page is all this
    // reads before the length says how far the request goes.
    let first = unsafe { core::slice::from_raw_parts(address as *const u8, 8) };
    let mut len = [0u8; 8];
    len.copy_from_slice(first);
    let len = usize::try_from(u64::from_le_bytes(len)).map_err(|_| "a bad length")?;
    if len + 8 > SHM_BYTES {
        return Err("the request is longer than its Shm");
    }
    // SAFETY: as above; the shell's Shm is SHM_BYTES long.
    let page = unsafe { core::slice::from_raw_parts_mut(address as *mut u8, SHM_BYTES) };
    let request = String::from_utf8_lossy(&page[8..8 + len]).into_owned();
    let host: Rc<dyn Host> = Rc::new(Child);
    let mut answer = shellcore::apart::serve(&request, &host, &shellcore::DEFAULT_CONFIG);
    answer.truncate(answer.floor_char_boundary(SHM_BYTES - 8));
    page[..8].copy_from_slice(&(answer.len() as u64).to_le_bytes());
    page[8..8 + answer.len()].copy_from_slice(answer.as_bytes());
    sys::shm_unmap(address).map_err(|_| "cannot unmap the request")?;
    Ok(())
}

/// The bytes of the shell's `Shm` (`user/src/bin/shell/apart.rs`: 8 pages).
const SHM_BYTES: usize = 8 * 4096;

fn moved_in(kind: Kind) -> Option<kabi::handle::Handle> {
    let mut list = [HandleInfo::default(); 8];
    let count = sys::startup(&mut list).ok()?.min(list.len());
    list[..count]
        .iter()
        .find(|h| h.kind() == Some(kind))
        .map(|h| kabi::handle::Handle(h.handle))
}

/// The OS as a script child sees it: only what its moved-in handles allow. It
/// asks no model and stops nothing; the shell does those.
struct Child;

impl Host for Child {
    fn now_ns(&self) -> u64 {
        sys::time_ns()
    }

    /// Ctrl-C reaches the shell, which ends this child.
    fn interrupted(&self) -> bool {
        false
    }

    fn heap_bytes(&self) -> u64 {
        user::heap_bytes()
    }

    fn metrics(&self) -> Result<kabi::Metrics, Error> {
        sys::metrics()
    }

    fn set_policy(&self, id: u64) -> Result<(), Error> {
        sys::set_policy(id).map(|_| ())
    }

    fn sched_stats(&self, window_ms: u64) -> Result<Box<SchedStats>, Error> {
        let mut stats = Box::new(SchedStats::empty());
        sys::sched_stats(&mut stats, window_ms)?;
        Ok(stats)
    }

    fn ask(&self, _: &str) -> Result<String, String> {
        Err(String::from("a script's process asks no model"))
    }

    fn script(&self, _: &ScriptRequest) -> Result<Answer, AnswerError> {
        Err(AnswerError::Server(String::from(
            "a script's process asks no model",
        )))
    }

    fn audit(&self, text: &str) {
        user::logln!("{text}");
    }
}

fn probe() -> Result<(), &'static str> {
    let mut list = [HandleInfo::default(); 8];
    let count = sys::startup(&mut list)
        .map_err(|_| "no STARTUP")?
        .min(list.len());
    let held = &list[..count];
    let find = |kind| {
        held.iter()
            .find(|h| h.kind() == Some(kind))
            .map(|h| kabi::handle::Handle(h.handle))
    };
    find(Kind::Sched).ok_or("no Sched handle moved in")?;
    let shm = find(Kind::Shm).ok_or("no Shm handle moved in")?;
    let mut stats = SchedStats::empty();
    sys::sched_stats(&mut stats, 0).map_err(|_| "the moved Sched handle did not work")?;
    // No capability bits: nothing it was not given.
    if sys::list(&mut [0u8; 64]) != Err(Error::Permission) {
        return Err("a supervisor call was not refused");
    }
    let address = sys::shm_map(shm, true).map_err(|_| "cannot map the result Shm")?;
    let line = alloc::format!("ok {count}");
    // SAFETY: SHM_MAP mapped at least one page read-write at `address`.
    let page = unsafe { core::slice::from_raw_parts_mut(address as *mut u8, 4096) };
    page[..line.len()].copy_from_slice(line.as_bytes());
    sys::shm_unmap(address).map_err(|_| "cannot unmap the result Shm")?;
    Ok(())
}
