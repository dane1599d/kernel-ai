//! Edit mode (M9b). `:edit notes` opens the running version's source in the ring-3
//! edit service; until `:quit`, lines that do not start with `:` go to it. It
//! builds on the host and stores the result as the next upload. `:apply` then
//! shows, from the kernel's own store, what changed between the active version's
//! source and the upload's, and swaps only on a yes typed at the kernel's prompt.
//!
//! Everything here returns text rather than printing, so the terminal and the
//! boot case run the same code.

use crate::process::components;
use crate::{ipc, log};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use editcore::ipc::{Ipc, encode_ipc};
use livecore::uploads;
use llmproto::text::printable;
use spin::Mutex;

/// The kernel's wait for the edit service: its build may take 150 s.
pub const EDIT_CALL_MS: u64 = 170_000;
/// The kernel's wait for a component's host to answer a swap.
const SWAP_CALL_MS: u64 = 60_000;
/// How long to wait for an edit service that is restarting.
const BIND_WAIT_NS: u64 = 3_000_000_000;
/// The screen size when there is no framebuffer console.
pub const FALLBACK_SCREEN: (usize, usize) = (160, 50);
const MAX_LINES: usize = 64;

pub const HELP: &[(&str, &str)] = &[
    (
        ":edit <name> [version]",
        "edit a component's source; plain lines then go to the editor",
    ),
    (
        ":apply [name]",
        "show the newest build's change and ask to run it",
    ),
    (
        ":revert <name> [n]",
        "swap back to the nth version that ran before",
    ),
    (":quit", "leave edit mode"),
];

/// The component being edited, if any.
static EDITING: Mutex<Option<String>> = Mutex::new(None);

pub fn editing() -> Option<String> {
    EDITING.lock().clone()
}

/// The prompt in edit mode.
pub fn prompt() -> Option<String> {
    editing().map(|name| format!("edit {name}> "))
}

/// Sends one request to the edit service; its reply, made printable, or why there
/// is none (as an `err` reply).
fn call_editor(message: &[u8]) -> String {
    // The supervisor restarts it within 1.6 s; longer means it is not coming back.
    let bound = crate::hpet::nanos() + BIND_WAIT_NS;
    if ipc::wait_bound("edit", bound, || false).is_err() {
        return String::from("err the edit service is not running (see :ps)");
    }
    match ipc::call("edit", message, EDIT_CALL_MS) {
        Ok(reply) => printable(
            &String::from_utf8_lossy(&reply),
            kabi::MAX_MESSAGE,
            MAX_LINES,
        ),
        Err(error) => format!("err the edit service did not answer ({error:?})"),
    }
}

/// `:edit <name> [version]`: opens (or resumes) a session; on success, lines go to
/// the editor from now on.
pub fn open(name: &str, label: Option<&str>) -> String {
    if crate::build_service::target().is_none() {
        return String::from("err no build service; boot with build=IP:PORT build_token=HEX");
    }
    let message = encode_ipc(&Ipc::Open {
        component: name,
        label,
    });
    let reply = call_editor(&message);
    if reply.starts_with("ok ") {
        *EDITING.lock() = Some(name.to_string());
    }
    reply
}

/// A line typed in edit mode, sent to the editor.
pub fn editor_line(line: &str) -> String {
    call_editor(&encode_ipc(&Ipc::Cmd(line)))
}

/// Whether an editor line will build on the host, which takes a while.
pub fn builds(line: &str) -> bool {
    line.trim() == "build"
}

/// `:quit`.
pub fn quit() -> String {
    match EDITING.lock().take() {
        Some(name) => {
            format!("ok left the editor; {name}'s session stays open in the edit service")
        }
        None => String::from("err not editing"),
    }
}

/// What `:apply` asks about.
#[derive(Clone, Debug)]
pub struct Proposal {
    pub component: String,
    pub label: String,
    /// The version the change was computed against.
    pub active: String,
    /// The lines to show before asking.
    pub lines: Vec<String>,
}

/// `:apply`: the newest upload and its change against the active version, both
/// read from the kernel's store, for a screen `cols` by `rows`.
pub fn propose(name: &str, cols: usize, rows: usize) -> Result<Proposal, String> {
    let component =
        components::component(name).ok_or_else(|| format!("no live component {name}"))?;
    // Each line is shown after a two-character lead.
    let shown = uploads::proposal(&component, cols.saturating_sub(2), rows)?;
    Ok(Proposal {
        component: name.to_string(),
        label: shown.label,
        active: shown.active,
        lines: shown.lines,
    })
}

/// The answer to a proposal: only `y` or `yes` swaps, as the kernel.
pub fn answer(proposal: &Proposal, answer: &str) -> String {
    if !matches!(answer.trim(), "y" | "yes") {
        return String::from("not applied");
    }
    let active = components::ledger(&proposal.component)
        .and_then(|ledger| ledger.active().map(String::from));
    if active.as_deref() != Some(proposal.active.as_str()) {
        return String::from("err the active version changed; :apply again");
    }
    swap(&proposal.component, &proposal.label)
}

fn swap(name: &str, label: &str) -> String {
    let request = format!("{}swap {label}", kabi::live::CONTROL_PREFIX);
    match ipc::call(name, request.as_bytes(), SWAP_CALL_MS) {
        Ok(reply) => printable(
            &String::from_utf8_lossy(&reply),
            kabi::MAX_MESSAGE,
            MAX_LINES,
        ),
        Err(error) => format!("err {name} did not answer ({error:?})"),
    }
}

/// `:revert <name> [n]`: swaps to the nth distinct version that was active before
/// the current one. Typing it is the confirmation.
pub fn revert(name: &str, n: usize) -> String {
    let Some(component) = components::component(name) else {
        return format!("err no live component {name}");
    };
    let Some(label) = component.ledger.revert_target(n) else {
        return format!("err {name} has no version {n} back");
    };
    if component.version(label).is_none() {
        return format!("err {name} {label} is no longer stored");
    }
    log!("live: reverting {name} to {label}");
    swap(name, label)
}

/// Prints a reply from a program with the leads only program text uses.
pub fn show(reply: &str) {
    let (lead, body) = match reply.split_once(' ').unwrap_or((reply, "")) {
        ("ok", body) => ("=", body),
        ("err", body) => ("!", body),
        _ => ("=", reply),
    };
    for line in body.lines() {
        log!("{lead} {line}");
    }
}

/// Runs the edit-mode kernel commands; None if `command` is not one.
pub fn command(command: &str, rest: &str) -> Option<Option<Proposal>> {
    let words: Vec<&str> = rest.split_whitespace().collect();
    let (cols, rows) = crate::console::size().unwrap_or(FALLBACK_SCREEN);
    let usage = |text: &str| log!("usage: {text}");
    match (command, words.as_slice()) {
        ("edit", [name]) => show(&open(name, None)),
        ("edit", [name, label]) => show(&open(name, Some(label))),
        ("edit", _) => usage(":edit <name> [version]"),
        ("quit", []) => show(&quit()),
        ("apply", []) | ("apply", [_]) => {
            let name = words.first().map(|s| s.to_string()).or_else(editing);
            let Some(name) = name else {
                usage(":apply <name>");
                return Some(None);
            };
            match propose(&name, cols, rows) {
                Ok(proposal) => {
                    for line in &proposal.lines {
                        log!("? {line}");
                    }
                    return Some(Some(proposal));
                }
                Err(why) => log!("! {why}"),
            }
        }
        ("revert", [name]) => show(&revert(name, 1)),
        ("revert", [name, n]) => match n.parse() {
            Ok(n) if (1..kabi::live::LINEAGE).contains(&n) => show(&revert(name, n)),
            _ => usage(":revert <name> [1 to 5]"),
        },
        ("revert" | "apply" | "quit", _) => usage(":apply [name], :revert <name> [n], :quit"),
        _ => return None,
    }
    Some(None)
}
