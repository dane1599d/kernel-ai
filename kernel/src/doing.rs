//! The kernel terminal's side of `do` (`termcore::doing`): a call consent let
//! through that only the terminal can run. The line is parsed into a closed
//! request, never re-typed, and runs the functions `:edit`, `:apply` and `:revert`
//! call. When the terminal showed a menu for the call, the request must carry the
//! `ONCE` that menu's Allow minted, and the kernel checks its digest here, so a
//! shell that skipped consent cannot drive these. Each `do` and its `done` go to
//! the logbook with the id.

use crate::edit_mode::{self, Proposal};
use crate::handle::grant as grants;
use crate::log;
use alloc::string::String;
use captable::once::Verdict;
use spin::Mutex;
use termcore::doing::{Do, Request, done};

/// The menu on screen and the call it shows, while the shell answers it.
static SHOWN: Mutex<Option<(u64, String)>> = Mutex::new(None);

/// The terminal drew menu `id`: a `do` for it must bring its ONCE.
pub fn menu_shown(id: u64, call: &str) {
    *SHOWN.lock() = Some((id, String::from(call)));
}

/// The shell has answered the menu.
pub fn menu_answered() {
    *SHOWN.lock() = None;
}

/// What a `do` came to: the `done` line for the shell, and for `apply` the change
/// the terminal now shows and asks about at its own prompt.
pub struct Done {
    pub line: String,
    pub apply: Option<Proposal>,
}

/// Runs one `do` line.
pub fn run(text: &str) -> Done {
    let id = text
        .split(' ')
        .nth(1)
        .and_then(|id| id.parse::<u64>().ok())
        .unwrap_or(0);
    // Logged only as plain text, so a line cannot add lines of its own.
    let plain =
        text.len() <= termcore::doing::MAX_DO && text.bytes().all(|b| (b' '..=b'~').contains(&b));
    match plain {
        true => log!("do {id}: {text}"),
        false => log!("do {id}: ({} bytes, not plain text)", text.len()),
    }
    let (result, apply) = match Request::decode(text) {
        Err(why) => (Err(String::from(why)), None),
        Ok(request) => match once_checks(&request) {
            Err(why) => (Err(why), None),
            Ok(()) => perform(request.what),
        },
    };
    let line = done(id, result.as_deref().map_err(String::as_str));
    log!("{line}");
    Done { line, apply }
}

/// A request for a call whose menu is on screen must carry that menu's ONCE, and
/// its digest must match the call as shown (`kai-once 2`); a match uses it up.
fn once_checks(request: &Request) -> Result<(), String> {
    let shown = SHOWN.lock().clone();
    let Some((id, call)) = shown.filter(|(id, _)| *id == request.id) else {
        return Ok(());
    };
    let badge = request
        .once
        .ok_or_else(|| String::from("the call was asked on the menu; its ONCE is missing"))?;
    match grants::check_once(badge, &termcore::once::digest_call(id, &call)) {
        Verdict::Match => Ok(()),
        other => Err(alloc::format!("its ONCE did not check ({other:?})")),
    }
}

fn perform(what: Do) -> (Result<String, String>, Option<Proposal>) {
    let (cols, rows) = crate::console::size().unwrap_or(edit_mode::FALLBACK_SCREEN);
    match what {
        Do::Edit {
            component,
            path,
            line,
        } => (edit_mode::open_at(&component, &path, line), None),
        Do::Apply { component } => match edit_mode::propose(&component, cols, rows) {
            Ok(proposal) => {
                for line in &proposal.lines {
                    log!("? {line}");
                }
                let text = alloc::format!(
                    "{component}: the change is shown; the terminal asks before it runs"
                );
                (Ok(text), Some(proposal))
            }
            Err(why) => (Err(why), None),
        },
        Do::Revert { component, back } => {
            let reply = edit_mode::revert(&component, usize::from(back));
            match reply.strip_prefix("ok ") {
                Some(text) => (Ok(String::from(text)), None),
                None => (
                    Err(String::from(reply.strip_prefix("err ").unwrap_or(&reply))),
                    None,
                ),
            }
        }
    }
}
