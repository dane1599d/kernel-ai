//! `test=ui-window-show` (the founder's second test, backlog item 2): a
//! closed app comes back when asked, and not before. `notes-window` opens its
//! window as in `ui-window`, and the case clicks its close control. A note
//! added through its action then redraws the app, which used to open the
//! window again unasked; now it stays closed, so the first `kai-show 1` finds
//! no window and reloads the app from its checkpoint, which shows it. The
//! second finds it open, with the note.

use crate::display::claim;
use crate::ui_tests::report;
use crate::{console, ipc, log, process, threads};
use alloc::string::String;

const NAME: &str = "notes-window";
const WAIT_MS: u64 = 60_000;
const CALL_MS: u64 = 60_000;
/// Time for comphost to start the app and paint its first frame under TCG.
const OPEN_MS: u64 = 10_000;
/// Where `desktop` starts the pointer: the middle of 1280x800.
const START: (i32, i32) = (640, 400);
/// The window's close control, as `ui-window` finds it.
const CLOSE: (i32, i32) = (48 + 642 - 17, 52 + 17);

fn call(request: &str) -> Result<String, &'static str> {
    ipc::call(NAME, request.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "ui-window-show: notes-window did not reply")
}

pub fn check() -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        let desktop = process::named("desktop").first().map(|&(pid, _)| pid);
        let shows = desktop.and_then(|pid| {
            process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid))
        });
        if shows.is_some_and(claim::shows) && !process::named(NAME).is_empty() {
            break;
        }
        threads::sleep_ms(50);
    }
    if !console::suspended() {
        return Err("ui-window-show: the desktop never took the screen");
    }
    threads::sleep_ms(OPEN_MS);
    report(CLOSE.0 - START.0, CLOSE.1 - START.1, 0);
    report(0, 0, 1);
    report(0, 0, 0);
    threads::sleep_ms(2_000);
    log!("ui-window-show: Notes' window was closed");
    // A redraw of its own: it used to open the window again.
    let add = "kai-call 1\nfunc app:notes/actions@1.0.0#add\nargs {text: \"buy milk\"}\n";
    if !call(add)?.contains("ok(1)") {
        return Err("ui-window-show: the note was not added");
    }
    threads::sleep_ms(2_000);
    let shown = call(kabi::live::SHOW)?;
    log!("ui-window-show: the first show answered {shown}");
    if shown != "ok shown notes-window" {
        return Err("ui-window-show: the closed window came back unasked, or no reload");
    }
    threads::sleep_ms(3_000);
    let again = call(kabi::live::SHOW)?;
    log!("ui-window-show: the second show answered {again}");
    if !again.contains("its window is open") {
        return Err("ui-window-show: the show did not bring the window back");
    }
    let count = "kai-call 1\nfunc app:notes/actions@1.0.0#count\nargs {}\n";
    if !call(count)?.contains("notes: 1") {
        return Err("ui-window-show: the note did not survive the show");
    }
    log!("ui-window-show: a closed window stayed closed until asked, then came back with its note");
    Ok(())
}
