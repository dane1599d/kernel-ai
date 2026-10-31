//! `app:monitor/actions@1.0.0` and `kai:system@0.1.0`'s bindings
//! (`apps/monitor/wit/monitor.wit`, world `bound`): each action reaches the one
//! instance through `compsdk::with`, moves the window's view and redraws it, and
//! a sample from the host is handed to the model.

use crate::MonitorWindow;
use alloc::string::String;
use alloc::vec::Vec;

wit_bindgen::generate!({
    world: "app:monitor/bound",
    path: ["../../wit/kai-system", "../monitor/wit"],
    generate_all,
});

use exports::app::monitor::actions::Guest as Actions;
use exports::kai::system::samples::Guest as Samples;

struct Bound;

fn started<R>(f: impl FnOnce(&mut MonitorWindow) -> R) -> R {
    match compsdk::with::<MonitorWindow, R>(f) {
        Some(result) => result,
        None => panic!("the monitor was called before it started"),
    }
}

/// Asks the host for a sample of the machine; its ticket.
pub fn read() -> u32 {
    kai::system::system::read()
}

impl Actions for Bound {
    fn sort(by: String) -> Result<String, String> {
        started(|app| app.act(|monitor| monitor.act_sort(&by)))
    }

    fn select(name: String) -> Result<String, String> {
        started(|app| app.act(|monitor| monitor.act_select(&name)))
    }
}

impl Samples for Bound {
    fn on_sample(_ticket: u32, outcome: Result<Vec<u8>, String>) {
        started(|app| app.sampled(outcome));
    }
}

export!(Bound);
