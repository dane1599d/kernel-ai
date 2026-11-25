//! `mode=reboot` (`test=power-reboot`, backlog item 13) and `mode=poweroff`
//! (`test=power-off`): POWER_REBOOT and POWER_OFF from ring 3. Without a
//! `Power` handle each is refused and nothing happens; with one (`cap::POWER`)
//! the machine restarts or turns off and this never returns.

use kabi::Error;
use kabi::handle::{HandleInfo, Kind, MAX_STARTUP, Rights};
use user::{println, sys};

/// How often, and how far apart, a Busy POWER_REBOOT is asked again.
const BUSY_TRIES: u32 = 100;
const BUSY_WAIT_MS: u64 = 100;

pub fn reboot() -> Result<(), &'static str> {
    ask("POWER_REBOOT", "asking for a restart", sys::power_reboot)
}

/// `mode=poweroff` (`test=power-off`, ADR 0007 section 8): POWER_OFF as
/// POWER_REBOOT is asked: refused without a `Power` handle, the machine off
/// with one.
pub fn power_off() -> Result<(), &'static str> {
    ask("POWER_OFF", "asking to turn off", sys::power_off)
}

/// Asks `call` (named `name`), saying `asking` first when a `Power` handle is
/// held; never returns when it works.
fn ask(name: &str, asking: &str, call: fn() -> Error) -> Result<(), &'static str> {
    let mut list = [HandleInfo::default(); MAX_STARTUP];
    let count = sys::startup(&mut list)
        .map_err(|_| "STARTUP failed")?
        .min(MAX_STARTUP);
    let power = list[..count]
        .iter()
        .find(|info| info.kind() == Some(Kind::Power));
    let Some(power) = power else {
        super::expect(
            Err::<(), _>(call()),
            Error::Permission,
            "a power call without a Power handle",
        )?;
        println!("power: {name} was refused without a Power handle");
        return Ok(());
    };
    if !power.rights().contains(Rights::WRITE) {
        return Err("the Power handle has no WRITE");
    }
    println!("power: {asking}");
    // Busy while the boot record is not yet written or `fs` holds a call: a
    // moment later it is not.
    let mut refused = call();
    for _ in 0..BUSY_TRIES {
        if refused != Error::Busy {
            break;
        }
        sys::sleep_ms(BUSY_WAIT_MS);
        refused = call();
    }
    println!("power: {name} returned {refused:?}");
    Err("a power call returned with a Power handle held")
}
