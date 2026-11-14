//! POWER_REBOOT: a restart from inside the OS (backlog item 13), for the holder
//! of [`kabi::cap::POWER`]. It resets through the firmware as an update's
//! restart does ([`crate::efi::reset`]). Turning the machine off needs ACPI's
//! \_S5, which comes later.

use super::need;
use kabi::{Error, call, cap};

/// [`call::POWER_REBOOT`], or `None` for a number this area does not have.
pub(super) fn handle(slot: usize, number: u64, _: &[u64; 6]) -> Option<Result<u64, Error>> {
    (number == call::POWER_REBOOT).then(|| reboot(slot))
}

fn reboot(slot: usize) -> Result<u64, Error> {
    need(slot, cap::POWER)?;
    let process = crate::process::identity(slot);
    crate::log!(
        "power: {} (pid {}) asked for a restart",
        process.name.as_str(),
        process.pid
    );
    crate::efi::reset()
}
