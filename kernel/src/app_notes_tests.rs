//! `test=app-notes` (P4's close, the agent-driven Notes case): Kai makes notes
//! in Notes' window, the window shows them, they are on disk, and `undo` takes
//! one back. Lines go to the shell as typed lines, and the mock model answers
//! each by its line (`xtask/src/mock_llm.rs`):
//!
//! 1. "add a note saying buy milk" runs Notes' `notes_add` over the app route.
//!    The text is in the person's own words, so the call runs without asking
//!    (its tier is Run, and its undo is the app's own `kai-undo 1`);
//! 2. "what do my notes say" runs `ui_view` on Notes' window, and the model's
//!    second round says what the view held: the note's text, read from the
//!    window's tree on `desktop`, not from the app;
//! 3. "add a note saying call mom" makes a second note, and `undo`, typed,
//!    takes it back: two notes, then one, and the window shows only the first.
//!
//! `notes-window` runs with `dir=apps`, so each checkpoint (the adds, and the
//! undo's) is kept in `/system/apps/notes-window/`. Boot 2 (`reread`), on the
//! same disk, starts from that saved state: one note, "buy milk", in the
//! window. Had the undo not been saved, boot 2 would find two.

use crate::app_editor_files_tests::shell;
use crate::display::claim;
use crate::{console, ipc, log, process, threads};
use alloc::string::String;

/// Notes' window, started at boot so its window is open, with its folder for
/// its saved state. Planned after the shell, as `desk-agent`'s is: the shell
/// reaches it through the supervisor's `app-call`.
pub const NOTES: &str = "notes-window program=comphost caps=serve+component+call uses=desktop dir=apps pages=8192 stack_pages=512 burst_ms=20000 version=1";

const NAME: &str = "notes-window";
const COUNT: &str = "kai-call 1\nfunc app:notes/actions@1.0.0#count\nargs {}\n";
const CALL_MS: u64 = 60_000;
const WAIT_MS: u64 = 120_000;
/// Time for comphost to start Notes and paint its first frame under TCG.
const OPEN_MS: u64 = 20_000;
/// The model's second round after "what do my notes say": what the view held.
const VIEW_LINE: &str = "what do my notes say";

/// Notes' own count, `notes: <n>, words: <m>`.
fn count() -> Result<String, &'static str> {
    ipc::call(NAME, COUNT.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "app-notes: notes-window did not answer")
}

fn expect_count(notes: u32, why: &'static str) -> Result<(), &'static str> {
    let tally = count()?;
    if !tally.contains(&alloc::format!("notes: {notes},")) {
        log!("app-notes: Notes counts {}", tally.replace('\n', "; "));
        return Err(why);
    }
    Ok(())
}

/// The shell's answer to `line`, which must start `ok `.
fn ok(line: &str) -> Result<String, &'static str> {
    let reply = shell(line)?;
    if !reply.starts_with("ok ") {
        log!("app-notes: {line:?} answered {reply:?}");
        return Err("app-notes: a line got the wrong answer");
    }
    Ok(reply)
}

/// What the agent read in the window: whether it showed each of the two notes.
fn shown() -> Result<(bool, bool), &'static str> {
    let said = ok(VIEW_LINE)?;
    log!("app-notes: the agent read the window: {}", said.trim_end());
    Ok((said.contains("buy milk"), said.contains("call mom")))
}

/// Waits for `desktop` to take the screen and Notes to run, then for its window.
fn desktop_and_notes() -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        let desktop = process::named("desktop").first().map(|&(pid, _)| pid);
        let shows = desktop.and_then(|pid| {
            process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid))
        });
        if shows.is_some_and(claim::shows) && ipc::server_of(NAME).is_some() {
            break;
        }
        threads::sleep_ms(50);
    }
    if !console::suspended() || ipc::server_of(NAME).is_none() {
        log!("app-notes: the desktop or Notes never started");
        return Err("app-notes: the desktop or Notes never started");
    }
    threads::sleep_ms(OPEN_MS);
    Ok(())
}

pub fn check(reread: bool) -> Result<(), &'static str> {
    desktop_and_notes()?;
    if reread {
        expect_count(1, "app-notes: boot 2 did not find the one note")?;
        if shown()? != (true, false) {
            return Err("app-notes: boot 2's window does not show only the first note");
        }
        log!("app-notes: boot 2's window shows the note Kai made, from the saved state");
        return Ok(());
    }
    let reply = ok("add a note saying buy milk")?;
    log!(
        "app-notes: Kai's notes_add ran at once, the text being the person's: {}",
        reply.trim_end()
    );
    expect_count(1, "app-notes: the note was not added")?;
    if shown()? != (true, false) {
        return Err("app-notes: the window does not show the note");
    }
    log!("app-notes: the Notes window shows the note Kai made");
    ok("add a note saying call mom")?;
    expect_count(2, "app-notes: the second note was not added")?;
    let undone = shell("undo")?;
    if !undone.starts_with("ok Undone") {
        log!("app-notes: undo answered {undone:?}");
        return Err("app-notes: undo did not take the second note back");
    }
    expect_count(1, "app-notes: undo did not take the second note back")?;
    if shown()? != (true, false) {
        return Err("app-notes: after undo the window does not show only the first note");
    }
    log!("app-notes: undo took the second note back, in Notes and its window");
    Ok(())
}
