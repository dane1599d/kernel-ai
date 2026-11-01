//! `test=update updated=models|probation` (P4 task 10): the model channel's
//! case, two boots on one data disk. `updated=models` imports the import
//! disk's fixture and switches to it; `updated=probation`, the next boot,
//! judges its trial. Neither restarts into a slot, so the kernel's half does
//! not wait for one as `test=update` otherwise does: `updated` leaves
//! `case-<step>.done` in `/system/updates` once its step ran and printed its
//! lines, and the boot passes once the marker is there. What the step printed
//! is xtask's to check.

use crate::{ipc, threads};
use alloc::format;
use alloc::vec::Vec;
use fsproto::{How, Reply, Request};
use kabi::handle::Rights;

const SERVICE: &str = "fs";
const CALL_MS: u64 = 10_000;
/// The fixture's import, or the trial's probation, with room.
const WAIT_MS: u64 = 200_000;
const POLL_MS: u64 = 100;

/// The model step this boot's `updated=` names, if it names one.
pub fn step() -> Option<&'static str> {
    crate::cmdline()
        .split_whitespace()
        .find_map(|w| w.strip_prefix("updated="))
        .filter(|step| matches!(*step, "models" | "probation"))
}

pub fn check(step: &str) -> Result<(), &'static str> {
    // The boot lever first, as every `test=update` boot: `updated` waits on it.
    crate::slots::on_boot(false);
    let marker = format!("case-{step}.done");
    for _ in 0..WAIT_MS / POLL_MS {
        if ipc::server_of(SERVICE).is_some() && opened(&marker) {
            return Ok(());
        }
        threads::sleep_ms(POLL_MS);
    }
    Err("model case: updated never left its step's marker")
}

/// Whether `/system/updates/<name>` exists, asked of `fs` by the
/// `SYSTEM_UPDATES` badge.
fn opened(name: &str) -> bool {
    let request = Request::Open {
        path: name.into(),
        rights: 0,
        how: How::Existing,
    };
    let bytes = fsproto::encode_request(&request);
    let deadline = crate::hpet::nanos() + CALL_MS * 1_000_000;
    let extra = ipc::Extra {
        badge: kabi::fs::SYSTEM_UPDATES,
        rights: (Rights::READ | Rights::ENUMERATE).bits(),
        handles: Vec::new(),
    };
    let Ok((reply, _, _)) = ipc::call_with(SERVICE, &bytes, 0, deadline, || false, extra) else {
        return false;
    };
    matches!(fsproto::decode_reply(&reply), Ok(Reply::Opened(..)))
}
