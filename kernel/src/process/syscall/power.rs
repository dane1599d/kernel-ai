//! POWER_REBOOT: a restart from inside the OS (backlog item 13), for the holder
//! of [`kabi::cap::POWER`], which only the shell may hold. It resets through the
//! firmware as an update's restart does, but only when nothing is half written:
//!
//! - Busy until this boot's lever has written its boot record
//!   ([`crate::slots::lever_done`]), as BOOTREC_SET is.
//! - Busy while `fs` is serving a call: `fs` commits each change before it
//!   answers (kaifs flushes the partition twice per commit), so between calls
//!   everything answered is on disk. An app's action left open across calls is
//!   lost, as on a power cut; the holder syncs or waits for its own first.
//! - A BOOTREC_SET under way writes both its variables first, and no firmware
//!   call runs during the reset ([`crate::slots::reset_when_set`]).
//!
//! Turning the machine off needs ACPI's \_S5, which comes later.

use super::need;
use kabi::{Error, call, cap};

/// [`call::POWER_REBOOT`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, _: &[u64; 6]) -> Option<Result<u64, Error>> {
    (number == call::POWER_REBOOT).then(|| reboot(slot))
}

fn reboot(slot: usize) -> Result<u64, Error> {
    need(slot, cap::POWER)?;
    if !crate::slots::lever_done() || fs_serving() {
        return Err(Error::Busy);
    }
    let process = crate::process::identity(slot);
    crate::log!(
        "power: {} (pid {}) asked for a restart",
        process.name.as_str(),
        process.pid
    );
    crate::slots::reset_when_set()
}

/// Whether the file system service holds a call now.
fn fs_serving() -> bool {
    crate::ipc::server_of(crate::process::supervision::FS)
        .and_then(crate::ipc::serving_deadline)
        .is_some()
}
