//! The Settings app's own interfaces on the host's side of the component
//! boundary: `app:settings/actions@1.0.0` (`apps/settings/wit/settings.wit`) and
//! the granted `kai:settings@0.1.0` (`wit/kai-settings/settings.wit`). Each
//! export reaches the one instance through `compsdk::with`.

use crate::SettingsWindow;
use alloc::string::String;
use alloc::vec::Vec;

mod actions {
    wit_bindgen::generate!({
        world: "app:settings/actions-only",
        path: ["../../wit/kai-live", "../../wit/kai-app", "../settings/wit"],
    });
    pub use exports::app::settings::actions::Guest;
}

mod access {
    wit_bindgen::generate!({
        world: "kai:settings/settings-access",
        path: ["../../wit/kai-settings"],
    });
    pub use exports::kai::settings::replies::Guest;
    pub use kai::settings::settings::send;
}

struct Bound;

fn started<R>(f: impl FnOnce(&mut SettingsWindow) -> R) -> Option<R> {
    compsdk::with::<SettingsWindow, R>(f)
}

impl actions::Guest for Bound {
    fn open(page: String) -> Result<String, String> {
        match started(|app| app.act_open(&page)) {
            Some(result) => result,
            None => panic!("settings was called before it started"),
        }
    }
}

impl access::Guest for Bound {
    fn on_reply(ticket: u32, outcome: Result<Vec<u8>, String>) {
        started(|app| app.on_reply(ticket, outcome));
    }
}

/// `kai:settings/settings.send`.
pub fn send(request: &str) -> u32 {
    access::send(request)
}

actions::export!(Bound with_types_in actions);
access::export!(Bound with_types_in access);
