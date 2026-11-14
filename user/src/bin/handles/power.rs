//! `mode=reboot` (`test=power-reboot`, backlog item 13): POWER_REBOOT from
//! ring 3. Without a `Power` handle it is refused and nothing happens; with
//! one (`cap::POWER`) the machine restarts and this never returns.

use kabi::Error;
use kabi::handle::{HandleInfo, Kind, MAX_STARTUP, Rights};
use user::{println, sys};

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
    let refused = sys::power_reboot();
    println!("power: POWER_REBOOT returned {refused:?}");
    Err("POWER_REBOOT returned with a Power handle held")
}
