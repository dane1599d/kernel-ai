//! The shell's window rows (P3 task 10, `deskcore::agent`): `desktop` answers
//! `ui ...` requests from the shell alone, against the desk and its clients'
//! trees. A key or click by the user in the window the agent is acting on stops
//! the run: the panel says so, and `deskcore::agent` refuses the run's later
//! acts until the shell's next request begins.

use super::{State, deliver, name_of};
use alloc::string::String;
use deskcore::agent::Clients;
use deskcore::desk::WindowId;
use deskcore::panel;
use deskcore::server::Client;
use surfabi::tree::Tree;
use user::println;

/// What the panel says when the user takes over.
pub const TOOK_OVER: &str = "Stopped: you took over";

/// `desktop`'s clients, as the agent's rows see them.
struct Windows<'a>(&'a mut [Client]);

impl Clients for Windows<'_> {
    fn tree(&self, window: WindowId) -> Option<&Tree> {
        self.0.iter().find_map(|client| client.tree(window))
    }

    fn private(&self, window: WindowId) -> bool {
        self.0.iter().any(|client| client.agent_private(window))
    }

    fn act(&mut self, window: WindowId, node: u32, action: u32, value: &str) -> bool {
        self.0
            .iter_mut()
            .find(|client| client.windows().any(|w| w == window))
            .is_some_and(|client| client.act(window, node, action, value))
    }
}

/// The shell's `ui ...` request: its answer, or `None` for anyone else's
/// message that is not one.
pub fn request(state: &mut State, bytes: &[u8], pid: u64) -> Option<String> {
    let text = core::str::from_utf8(bytes).ok()?;
    if !text.starts_with("ui ") {
        return None;
    }
    if name_of(pid) != "shell" {
        return Some(String::from("err only the shell acts for the agent"));
    }
    let mut windows = Windows(&mut state.clients);
    let (answer, effects) = state.agent.answer(&mut state.desk, &mut windows, text);
    // Its verb only: a value may hold anything the model wrote.
    let verb = text.split([' ', '\t']).nth(1).unwrap_or("");
    let outcome = if answer.starts_with("err") {
        "refused"
    } else {
        "ok"
    };
    println!("desktop: the agent's ui {verb}: {outcome}");
    deliver(state, &effects);
    Some(answer)
}

/// The user acted in the window the agent was driving: the run stops, and the
/// panel says so.
pub fn took_over(state: &mut State, window: WindowId) {
    state.agent.took_over();
    println!(
        "desktop: the user took over w{}; the panel says: {TOOK_OVER}",
        window.0
    );
    let reason = String::from(TOOK_OVER);
    if state.panel.set_state(panel::State::Refused { reason }) {
        state.extra.push(state.desk.panel());
    }
}
