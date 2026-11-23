//! POWER_REBOOT and POWER_OFF: a restart or a shutdown from inside the OS
//! (backlog item 13, ADR 0007 section 8), for the holder of
//! [`kabi::cap::POWER`], which only the shell may hold. Each goes through the
//! firmware's ResetSystem, as an update's restart does, but only when nothing
//! is half written:
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
//! POWER_OFF asks ResetSystem for a shutdown; without it the console says it
//! is safe to switch off. ACPI's \_S5 (uACPI) comes later, ahead of both.

use super::need;
use kabi::{Error, call, cap};

/// [`call::POWER_REBOOT`] or [`call::POWER_OFF`], or `None` for a number this
/// area does not have.
pub(super) fn handle(slot: usize, number: u64, _: &[u64; 6]) -> Option<Result<u64, Error>> {
    match number {
        call::POWER_REBOOT => Some(power(slot, Kind::Restart)),
        call::POWER_OFF => Some(power(slot, Kind::Off)),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Restart,
    Off,
}

fn power(slot: usize, kind: Kind) -> Result<u64, Error> {
    need(slot, cap::POWER)?;
    if !crate::slots::lever_done() || fs_serving() {
        return Err(Error::Busy);
    }
    let process = crate::process::identity(slot);
    let what = match kind {
        Kind::Restart => "a restart",
        Kind::Off => "the computer off",
    };
    crate::log!(
        "power: {} (pid {}) asked for {what}",
        process.name.as_str(),
        process.pid
    );
    match kind {
        Kind::Restart => crate::slots::reset_when_set(),
        Kind::Off => crate::slots::power_off_when_set(),
    }
}

/// Whether the file system service holds a call now.
fn fs_serving() -> bool {
    crate::ipc::server_of(crate::process::supervision::FS)
        .and_then(crate::ipc::serving_deadline)
        .is_some()
}
