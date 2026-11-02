//! The shell's routes after `updated`, found by the names its plan line's
//! `routes=` gives them (`kabi::plan::named_route`): `desktop` on a desktop,
//! `pkg` where `cc build` installs. A route is found by its name, never by its
//! place, so a plan line that adds one moves no other.

use alloc::string::String;
use kabi::handle::Handle;
use spin::Once;

static NAMES: Once<String> = Once::new();

/// The route named `name`, if this shell's plan line gives one.
pub fn named(name: &str) -> Option<Handle> {
    let names = NAMES.call_once(|| {
        let args = user::args();
        String::from(user::arg(&args, "routes").unwrap_or(""))
    });
    user::route(kabi::plan::named_route(names, name)?)
}
