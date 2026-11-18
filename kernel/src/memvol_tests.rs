//! `test=memory-volume` (agents-os.md M1, ADR 0013 §3): a purged record is in
//! no snapshot after a reboot. The kernel asks `fs` on its plain route, which
//! only the kernel reaches, since no badge names the `memory` volume:
//! - boot 1: `fs` makes a record in `memory` in one action, takes a named
//!   snapshot over it, forgets it in a second action and purges it, which
//!   drops the two snapshots that held it;
//! - boot 2 (`reread`, same disk): no snapshot of `memory` holds any object
//!   but its root folder, and only the one from before the record is left.

use crate::{ipc, log};
use alloc::string::String;
use alloc::vec::Vec;
use fsproto::kernel_route::{MEMORY_CHECK, MEMORY_PROBE};

const SERVICE: &str = "fs";
const CALL_MS: u64 = 30_000;
/// The plan: `fs` alone.
pub const PLAN: &str = kabi::plan::FS;

pub fn check(cmdline: &str) -> Result<(), &'static str> {
    let reread = cmdline.split_whitespace().any(|w| w == "reread");
    let (request, want, line) = match reread {
        false => (
            MEMORY_PROBE,
            "ok purged 2",
            "memory-volume: boot 1 forgot a record and purged the 2 snapshots holding it",
        ),
        true => (
            MEMORY_CHECK,
            "ok snapshots=1 holding=0",
            "memory-volume: boot 2: no snapshot of memory holds the purged record",
        ),
    };
    let reply = ask(request)?;
    if reply != want.as_bytes() {
        log!(
            "memory-volume: fs answered {:?}",
            String::from_utf8_lossy(&reply)
        );
        return Err("memory-volume: fs's answer was not the one wanted");
    }
    log!("{line}");
    Ok(())
}

fn ask(request: &[u8]) -> Result<Vec<u8>, &'static str> {
    ipc::call(SERVICE, request, CALL_MS).map_err(|_| "memory-volume: the call to fs failed")
}
