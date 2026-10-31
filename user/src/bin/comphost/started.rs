//! `started` (`kai:app/events`), for every app comphost runs: the shown
//! version's instance is told once, when it is first shown, and an app asks
//! then for what it shows (the Settings app reads its pages). A reloaded,
//! swapped or resumed instance is a new one, with a serial of its own, so it is
//! told again; a standby is never told, as its requests are dropped, and a
//! suspended app is not woken for it.

use livecore::Performed;
use livecore::host::{Env, Host};
use user::println;
use wasmhost::{AppEvent, LiveInstance};

#[derive(Default)]
pub struct Started {
    /// The serial of the instance last told.
    told: Option<u64>,
}

impl Started {
    /// Tells the shown instance it started, if it has not been told; what its
    /// transaction did, for the caller to send on.
    pub fn tell<E: Env<Instance = LiveInstance>>(
        &mut self,
        host: &mut Host<E>,
    ) -> Option<Performed> {
        if host.suspended() {
            return None;
        }
        let serial = shown_serial(host)?;
        if self.told == Some(serial) {
            return None;
        }
        self.told = Some(serial);
        let performed = crate::ui::serve_shown(host, |core| core.deliver_event(&AppEvent::Started));
        if performed.is_none() {
            println!("comphost: the app could not be told it started");
        }
        performed
    }
}

/// The serial of the shown version's instance, if one is loaded.
pub fn shown_serial<E: Env<Instance = LiveInstance>>(host: &mut Host<E>) -> Option<u64> {
    let mut serial = None;
    host.each_instance(|instance, _, shown| {
        if let (LiveInstance::V2(core), true) = (instance, shown) {
            serial = Some(core.serial());
        }
    });
    serial
}
