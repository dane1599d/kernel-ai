//! `scriptrun` (ADR 0001 D6): a script's own process. The agent core starts it
//! with JOB_SPAWN, holding no capability bits: only the run-tier handles moved in
//! under `request N`, so revoking that node leaves it nothing.
//!
//! This is the kernel's half (P1 task 14): `mode=probe` reads its Sched handle,
//! checks that a call needing a handle it was not given is refused, and writes
//! `ok <handles>` to the start of the `Shm` it was given. The script engine and
//! the `actlog 1` result are A-core's half.

#![no_std]
#![no_main]

use kabi::Error;
use kabi::handle::{HandleInfo, Kind};
use kabi::sched::SchedStats;
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
        _ => {
            println!("scriptrun: no script engine here yet (mode=probe only)");
            2
        }
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

extern crate alloc;
