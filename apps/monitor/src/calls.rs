//! The one OS row the monitor asks for: `process_stop`, as an effect (ADR 0003
//! section 4). What the monitor shows it reads from a [`crate::Sample`] the host
//! hands in (`kai:system`), not from rows, so stopping is the only thing that
//! leaves the app, and the agent core gives it its tier: it asks, and nothing
//! undoes it.

use alloc::format;
use alloc::string::String;

/// One effect: an OS row by name and its arguments (a WAVE record).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub name: &'static str,
    pub args: String,
}

/// `process_stop`: stops or kills processes by name. `None` for a name the row
/// would not take.
pub fn stop(name: &str) -> Option<Call> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name
            .chars()
            .any(|c| c.is_control() || c == '"' || c == '\\');
    ok.then(|| Call {
        name: "process_stop",
        args: format!("{{name: \"{name}\"}}"),
    })
}
