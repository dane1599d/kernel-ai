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
use core::sync::atomic::{AtomicBool, Ordering};
use editcore::journal::Journal;
use llmproto::text::printable;
use spin::Mutex;

/// How often a waiting exchange looks at whether it is its turn.
const TURN_POLL_MS: u64 = 2;
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
    match exchange(message, &|| false) {
        Ok(reply) => {
            // The line saying a build moved the cursor is for the journal alone.
            let shown = editcore::ipc::split_moved(&reply).1;
            printable(
                &String::from_utf8_lossy(shown),
                kabi::MAX_MESSAGE,
                MAX_LINES,
            )
        }
        Err(why) => why,
    }
}

/// As [`call`], for an app that holds the `edit` grant: the reply as the
/// service sent it, with the line a build moved the cursor (`#at path:line`), at
/// most [`kabi::MAX_MESSAGE`] bytes. The service cleans what it sends, and the
/// editor app draws it through `uikit`, which draws no control character.
/// `cancel` ends the wait when the caller is killed or stopped.
pub fn call_for_app(message: &[u8], cancel: &dyn Fn() -> bool) -> String {
    match exchange(message, cancel) {
        Ok(reply) => {
            let mut text = String::from_utf8_lossy(&reply).into_owned();
            text.truncate(text.floor_char_boundary(kabi::MAX_MESSAGE));
            text
        }
        Err(why) => why,
    }
}

/// Whose turn it is to talk to the edit service. The console's edit mode and an
/// app both come here, and the journal must hold the messages in the order the
/// service answered them, with a restart's replay finished before the next
/// message, so only one exchange runs at a time. A flag, not a lock the state's
/// lock could be taken inside: [`built`], which runs while an exchange waits,
/// does not need it.
static TURN: AtomicBool = AtomicBool::new(false);

/// Holds [`TURN`] until dropped.
struct Turn;

impl Turn {
    /// Waits for the turn; None when `cancel()` says to stop waiting.
    fn take(cancel: &dyn Fn() -> bool) -> Option<Self> {
        while TURN
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            if cancel() {
                return None;
            }
            threads::sleep_ms(TURN_POLL_MS);
        }
        Some(Self)
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        TURN.store(false, Ordering::Release);
    }
}

/// The exchange itself: the service's reply, or why there is none. The journal
/// records what was answered.
fn exchange(message: &[u8], cancel: &dyn Fn() -> bool) -> Result<Vec<u8>, String> {
    let Some(_turn) = Turn::take(cancel) else {
        return Err(String::from("err cancelled"));
    };
    // The supervisor restarts it within 1.6 s; longer means it is not coming back.
    let bound = hpet::nanos() + BIND_WAIT_NS;
    if let Err(error) = ipc::wait_bound("edit", bound, cancel) {
        return Err(match error {
            ipc::CallError::Cancelled => String::from("err cancelled"),
            _ => String::from("err the edit service is not running (see :ps)"),
        });
    }
    restore_if_new(cancel);
    let answered = ipc::call_from_unless("edit", message, edit_mode::EDIT_CALL_MS, cancel);
    let (reply, server) = match answered {
        Ok(answered) => answered,
        // No answer: not journaled, so a command that kills the service is never
        // replayed into the next one.
        Err(ipc::CallError::Cancelled) => return Err(String::from("err cancelled")),
        Err(error) => return Err(format!("err the edit service did not answer ({error:?})")),
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
    Ok(reply)
}

/// COMP_PUT stored `component` `label`, built from `base`; the journal notes the
/// session as built when it is based there ([`Journal::built`] decides). Runs in
/// the edit service's COMP_PUT, perhaps while [`call`] waits for that service:
/// [`call`] holds no lock while it waits, and this takes only the journal's.
pub fn built(component: &str, base: &str, label: &str) {
    if let Some(warning) = with(|state| state.journal.built(component, base, label)) {
        log!("edit: {warning}");
    }
}

/// Gives a new edit service the journal's session, if there is one.
fn restore_if_new(cancel: &dyn Fn() -> bool) {
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
        let (reply, server) =
            ipc::call_from_unless("edit", message, edit_mode::EDIT_CALL_MS, cancel).ok()?;
        taker = server;
        Some(reply)
    };
    let restore = journal.restore(&mut send);
    let notice = printable(&restore.notice, kabi::MAX_MESSAGE, 1);
    log!("edit: {notice}");
    let new = with(|state| {
        // No upload lands meanwhile: the new service takes a build's result only
        // after an open or a command, which comes after this replay.
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
