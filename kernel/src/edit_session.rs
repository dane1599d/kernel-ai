//! The kernel's side of the edit session (M9d). Every open and command for the
//! ring-3 edit service goes through [`call`], which keeps the session journal
//! (`editcore::journal`): the open and each command answered `ok`. When a new
//! process serves `edit`, the journal is replayed into it before the next
//! message, so an editor crash loses nothing that was answered. The version the
//! session is based on is pinned in the store, so eviction cannot drop it.

use crate::process::components;
use crate::{edit_mode, hpet, ipc, log, threads};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use editcore::journal::Journal;
use llmproto::text::printable;
use spin::Mutex;

/// How long to wait for an edit service that is restarting.
const BIND_WAIT_NS: u64 = 3_000_000_000;
/// Most lines one reply prints.
const MAX_LINES: usize = 64;

struct State {
    journal: Journal,
    /// The edit service the journal was last given to; 0 before any.
    pid: u64,
    /// What the last restore could not bring back, as typed.
    lost: Vec<String>,
    /// What the last restore said.
    restored: Option<String>,
}

static STATE: Mutex<State> = Mutex::new(State {
    journal: Journal::new(),
    pid: 0,
    lost: Vec::new(),
    restored: None,
});

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    threads::preempt_disable();
    let result = f(&mut STATE.lock());
    threads::preempt_enable();
    result
}

/// The component and version the session is based on.
fn base(journal: &Journal) -> Option<(String, String)> {
    let (component, label) = journal.session()?;
    Some((component.to_string(), label.to_string()))
}

/// Moves the store's pin from the old base to the new one.
fn repin(old: Option<(String, String)>, new: Option<(String, String)>) {
    if old == new {
        return;
    }
    if let Some((component, _)) = &old {
        components::pin(component, None);
    }
    if let Some((component, label)) = &new {
        components::pin(component, Some(label));
    }
}

/// Sends one open or command to the edit service; its reply, made printable, or
/// why there is none (as an `err` reply). The journal records what was answered.
pub fn call(message: &[u8]) -> String {
    // The supervisor restarts it within 1.6 s; longer means it is not coming back.
    let bound = hpet::nanos() + BIND_WAIT_NS;
    if ipc::wait_bound("edit", bound, || false).is_err() {
        return String::from("err the edit service is not running (see :ps)");
    }
    restore_if_new();
    let (reply, server) = match ipc::call_from("edit", message, edit_mode::EDIT_CALL_MS) {
        Ok(answered) => answered,
        // No answer: not journaled, so a command that kills the service is never
        // replayed into the next one.
        Err(error) => return format!("err the edit service did not answer ({error:?})"),
    };
    let (old, recorded, new) = with(|state| {
        let old = base(&state.journal);
        // A newer service that never got the journal answered; it gets the
        // journal before the next message, so its reply is not journaled.
        let stray = server != state.pid && !state.journal.is_empty();
        let recorded = (!stray).then(|| {
            state.pid = server;
            state.journal.record(message, &reply)
        });
        let new = base(&state.journal);
        if old != new {
            state.lost.clear();
        }
        (old, recorded, new)
    });
    repin(old, new);
    match recorded {
        None => log!(
            "edit: the edit service restarted before it answered; the next line gives it the session back"
        ),
        Some(Some(warning)) => log!("edit: {warning}"),
        Some(None) => {}
    }
    printable(
        &String::from_utf8_lossy(&reply),
        kabi::MAX_MESSAGE,
        MAX_LINES,
    )
}

/// Gives a new edit service the journal's session, if there is one.
fn restore_if_new() {
    let Some(pid) = ipc::server_of("edit") else {
        return;
    };
    let journal = with(|state| {
        let new = core::mem::replace(&mut state.pid, pid) != pid;
        (new && !state.journal.is_empty()).then(|| state.journal.clone())
    });
    let Some(mut journal) = journal else {
        return;
    };
    let old = base(&journal);
    // The service that answered last holds what was replayed.
    let mut taker = pid;
    let mut send = |message: &[u8]| {
        let (reply, server) = ipc::call_from("edit", message, edit_mode::EDIT_CALL_MS).ok()?;
        taker = server;
        Some(reply)
    };
    let restore = journal.restore(&mut send);
    let notice = printable(&restore.notice, kabi::MAX_MESSAGE, 1);
    log!("edit: {notice}");
    let new = with(|state| {
        state.journal = journal;
        // Not answered: the next message tries again.
        state.pid = if restore.again { 0 } else { taker };
        state.lost = restore.lost;
        state.restored = Some(notice);
        base(&state.journal)
    });
    repin(old, new);
}

/// What the kernel said when it last restored a session, for the boot case.
pub fn restored() -> Option<String> {
    with(|state| state.restored.clone())
}

/// `:journal`: the journal as it stands, and what the last restore could not
/// bring back.
pub fn journal() -> String {
    let (lost, commands, base, edits) = with(|state| {
        (
            state.lost.clone(),
            state.journal.commands(),
            base(&state.journal),
            state.journal.counted(),
        )
    });
    let listed = |lines: &[String]| -> String {
        let shown = lines
            .iter()
            .map(|line| printable(line, kabi::MAX_MESSAGE, 1));
        shown.map(|line| format!("\n  {line}")).collect()
    };
    let mut out = match &base {
        Some((component, label)) => format!(
            "ok the journal of {component} {label}, {edits}; a restarted edit service gets these back:{}",
            listed(&commands)
        ),
        None if lost.is_empty() => {
            return String::from("err nothing is journaled; :edit <name> opens a session");
        }
        None => String::from("ok nothing is journaled"),
    };
    if !lost.is_empty() {
        out.push_str("\nthe last restart did not bring these back; type them again:");
        out.push_str(&listed(&lost));
    }
    out
}
