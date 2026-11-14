//! `mode=reboot` (`test=power-reboot`, backlog item 13): POWER_REBOOT from
//! ring 3. Without a `Power` handle it is refused and nothing happens; with
//! one (`cap::POWER`) the machine restarts and this never returns.

use kabi::Error;
use kabi::handle::{HandleInfo, Kind, MAX_STARTUP, Rights};
use user::{println, sys};

/// How often, and how far apart, a Busy POWER_REBOOT is asked again.
const BUSY_TRIES: u32 = 100;
const BUSY_WAIT_MS: u64 = 100;

pub fn reboot() -> Result<(), &'static str> {
    let mut list = [HandleInfo::default(); MAX_STARTUP];
    let count = sys::startup(&mut list)
        .map_err(|_| "STARTUP failed")?
        .min(MAX_STARTUP);
    let power = list[..count]
        .iter()
        .find(|info| info.kind() == Some(Kind::Power));
    let Some(power) = power else {
        super::expect(
            Err::<(), _>(sys::power_reboot()),
            Error::Permission,
            "POWER_REBOOT without a Power handle",
        )?;
        println!("power: POWER_REBOOT was refused without a Power handle");
        return Ok(());
    };
    if !power.rights().contains(Rights::WRITE) {
        return Err("the Power handle has no WRITE");
    }
    println!("power: asking for a restart");
    // Busy while the boot record is not yet written or `fs` holds a call: a
    // moment later it is not.
    let mut refused = sys::power_reboot();
    for _ in 0..BUSY_TRIES {
        if refused != Error::Busy {
            break;
        }
        sys::sleep_ms(BUSY_WAIT_MS);
        refused = sys::power_reboot();
    }
    println!("power: POWER_REBOOT returned {refused:?}");
    Err("POWER_REBOOT returned with a Power handle held")
}
