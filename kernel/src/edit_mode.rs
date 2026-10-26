//! Edit mode (M9b). `:edit notes` opens the running version's source in the ring-3
//! edit service; until `:quit`, lines that do not start with `:` go to it. It
//! builds on the host and stores the result as the next upload. `:apply` then
//! shows, from the kernel's own store, what changed between the active version's
//! source and the upload's, and swaps only on a yes typed at the kernel's prompt.
//!
//! Everything here returns text rather than printing, so the terminal and the
//! boot case run the same code. Messages to the editor go through
//! [`edit_session`], which journals them.

use crate::edit_session::{self, call as call_editor};
use crate::process::components;
use crate::{ipc, log};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use editcore::ipc::{At, Ipc, encode_ipc};
use livecore::uploads;
use llmproto::text::printable;
use spin::Mutex;

/// The kernel's wait for the edit service. Builds run in the background, so its
/// longest answer is `try` (COMP_CONTROL, 60 s), maybe behind a 30 s git sync.
pub const EDIT_CALL_MS: u64 = 100_000;
/// The kernel's wait for a component's host to answer a swap.
const SWAP_CALL_MS: u64 = 60_000;
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
    (":fault [name]", "open the source at the latest trap's line"),
    (
        ":journal",
        "the edits a restarted editor gets back, or did not",
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

/// `:edit <name> [version]`: opens (or resumes) a session; on success, lines go to
/// the editor from now on.
pub fn open(name: &str, label: Option<&str>) -> String {
    if crate::build_service::target().is_none() {
        return String::from("err no build service; boot with build=IP:PORT build_token=HEX");
    }
    let message = encode_ipc(&Ipc::Open {
        component: name,
        label,
        at: None,
    });
    let reply = call_editor(&message);
    if reply.starts_with("ok ") {
        *EDITING.lock() = Some(name.to_string());
    }
    reply
}

/// Opens the running version of `name` in the editor at `path`:`line`, for the
/// shell's `do edit` (`crate::doing`). The file must be in that version's stored
/// source and the line within it; the kernel checks both from its own store
/// before the editor is asked.
pub fn open_at(name: &str, path: &str, line: u32) -> Result<String, String> {
    let component =
        components::component(name).ok_or_else(|| format!("no live component {name}"))?;
    let label = component
        .ledger
        .active()
        .ok_or_else(|| format!("{name} has no running version"))?;
    let bytes = component
        .bytes(label)
        .map_err(|_| format!("{name} {label} is no longer stored"))?;
    let artifact = livecore::artifact::parse(bytes.as_slice())?;
    let files = livecore::artifact::source_files(artifact.source)?;
    let (_, text) = files
        .iter()
        .find(|(file, _)| *file == path)
        .ok_or_else(|| format!("{name} {label} has no file {path}"))?;
    let lines = text.split(|&b| b == b'\n').count() - usize::from(text.ends_with(b"\n"));
    if !(1..=lines).contains(&(line as usize)) {
        return Err(format!("{path} has {lines} lines, not {line}"));
    }
    if crate::build_service::target().is_none() {
        return Err(String::from(
            "no build service; boot with build=IP:PORT build_token=HEX",
        ));
    }
    let message = encode_ipc(&Ipc::Open {
        component: name,
        label: None,
        at: Some(At { path, line }),
    });
    let reply = call_editor(&message);
    match reply.strip_prefix("ok ") {
        Some(text) => {
            *EDITING.lock() = Some(name.to_string());
            Ok(text.to_string())
        }
        None => Err(reply.strip_prefix("err ").unwrap_or(&reply).to_string()),
    }
}

/// `:fault [name]`: the version whose trap last named a line, opened in the editor
/// at that line. With no build service, or when the editor cannot open it, the
/// lines around it from the kernel's own store instead.
pub fn fault(name: &str) -> String {
    let Some(component) = components::component(name) else {
        return format!("err no live component {name}");
    };
    let refused = match &component.ledger.last_located {
        Some(located) if crate::build_service::target().is_some() => {
            let message = encode_ipc(&Ipc::Open {
                component: name,
                label: Some(&located.label),
                at: Some(At {
                    path: &located.at.path,
                    line: located.at.line,
                }),
            });
            let reply = call_editor(&message);
            if reply.starts_with("ok ") {
                *EDITING.lock() = Some(name.to_string());
                return reply;
            }
            Some(reply)
        }
        _ => None,
    };
    let (cols, _) = crate::console::size().unwrap_or(FALLBACK_SCREEN);
    let width = crate::terminal::PIECE_CHARS.min(cols.saturating_sub(2));
    match livecore::location::fault_lines(&component, width) {
        // Unbuilt edits are the founder's to keep: say so, rather than show lines.
        _ if refused.as_deref().is_some_and(|r| r.contains("not built")) => {
            refused.unwrap_or_default()
        }
        Ok(lines) => format!("ok {}", lines.join("\n")),
        Err(why) => format!("err {why}"),
    }
}

/// A line typed in edit mode, sent to the editor.
pub fn editor_line(line: &str) -> String {
    call_editor(&encode_ipc(&Ipc::Cmd(line)))
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

pub(crate) fn swap(name: &str, label: &str) -> String {
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
        ("journal", []) => show(&edit_session::journal()),
        ("fault", [name]) => show(&fault(name)),
        ("fault", []) => match editing() {
            Some(name) => show(&fault(&name)),
            None => usage(":fault <name>"),
        },
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
        ("revert" | "apply" | "quit" | "fault" | "journal", _) => {
            usage(":apply [name], :revert <name> [n], :fault [name], :journal, :quit")
        }
        _ => return None,
    }
    Some(None)
}
