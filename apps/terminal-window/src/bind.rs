//! `app:terminal-window/actions@1.0.0`'s bindings (`wit/window.wit`, world
//! `actions-only`): each function reaches the one instance through
//! `compsdk::with`, runs on the model and redraws the window.

use crate::TerminalWindow;
use alloc::string::String;

wit_bindgen::generate!({
    world: "app:terminal-window/actions-only",
    path: [
        "../../wit/kai-live",
        "../../wit/kai-app",
        "../../wit/kai-ui",
        "wit",
    ],
});

use exports::app::terminal_window::actions::Guest;

struct Bound;

fn started<R>(f: impl FnOnce(&mut TerminalWindow) -> R) -> R {
    match compsdk::with::<TerminalWindow, R>(f) {
        Some(result) => result,
        None => panic!("the terminal was called before it started"),
    }
}

impl Guest for Bound {
    fn run(line: String) -> Result<String, String> {
        started(|app| app.type_line(&line))
    }

    fn read(since: u32) -> Result<String, String> {
        started(|app| app.term.act_read(since))
    }

    fn clear() -> Result<String, String> {
        started(|app| app.act(|term, _| term.act_clear()))
    }
}

export!(Bound);
