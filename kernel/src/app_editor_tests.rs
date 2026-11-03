//! `test=app-editor` (P4 task 11, the editor): the editor app on the desktop,
//! over the edit service and the host's build service.
//!
//! comphost runs the `editor` component, which the kernel grants `kai:edit` to by
//! that name, with a route to `desktop` for its window. The case calls the app's
//! `open` action at the door the agent's `editor_open` row reaches (`app-route`
//! covers the shell's side of it) and waits for the window to show Notes'
//! `src/lib.rs` at the line. It clicks into the window, and the host types a
//! comment (`// src/lib.rs:6`, which holds a source link) and Ctrl-B: the app sends
//! the typed text and `build` to the edit service from the key's call, so the
//! build gate takes it as the person's, and the host builds in the background; the
//! step ends when the build's upload is in the component store. Last the host holds
//! Ctrl while the case clicks on the link, and the editor shows the linked line.
//!
//! Nothing here waits for a time: each step of the host's keys (a stage in the
//! case's table, sent when this test logs `ready for keys`) is waited for in the
//! editor's own status, which shows the cursor and the edits, so a slow guest
//! (TCG, a painting desktop) only takes longer. Each step shows in comphost's,
//! `desktop`'s and the edit service's log lines, which the case in
//! `xtask/src/cases/components.rs` expects.

use crate::display::claim;
use crate::process::components;
use crate::ui_tests::report;
use crate::{build_service, console, ipc, log, process, threads};
use alloc::format;
use alloc::string::String;

/// The plan lines this case adds, after the `llm` and shell lines every case has:
/// `desktop` first, as the editor's route to it must be listed before it; Notes
/// at version 2, which `:edit notes` edits; and the edit service, which exists
/// only when the host has a build service.
pub fn plan() -> String {
    let mut plan = format!(
        "desktop caps=display+input+serve pages=8192 stack_pages=512\n{}",
        kabi::plan::NOTES_EDIT_TEST
    );
    if let Some(edit) = build_service::plan_line() {
        plan.push('\n');
        plan.push_str(&edit);
    }
    plan.push('\n');
    plan.push_str(kabi::plan::EDITOR);
    plan
}

pub(crate) const NAME: &str = "editor";
const CALL_MS: u64 = 60_000;
/// For a step to show in the editor, under TCG.
pub(crate) const WAIT_MS: u64 = 120_000;
/// A build on the host, from the key to the upload, under TCG.
const BUILD_MS: u64 = 200_000;
/// Time for comphost to start the app and paint its first frame under TCG.
pub(crate) const OPEN_MS: u64 = 10_000;
/// Time for the host's Ctrl to be down before the click: the harness answers the
/// log line and QEMU queues the key at once, so this is long enough on any guest,
/// and the hold outlasts the three reports.
const CTRL_LEAD_MS: u64 = 2_000;
/// Between two looks at the app's status, each of which also lets it look at a
/// running build.
const POLL_MS: u64 = 2_000;
/// Where `desktop` starts the pointer: the middle of 1280x800.
const START: (i32, i32) = (640, 400);
/// Inside the code view's first rows, which hold Notes' `//!` lines (1 to 4), so a
/// comment typed at the click is still a comment: the window is the first, at
/// 48,52, under its 34 px title bar and the app's own title line, and the click is
/// well right of the gutter and within the lines' text.
const CODE: (i32, i32) = (48 + 300, 52 + 100);
/// How many keys each of the case's typing stages types (`// sr`, `c/lib`,
/// `.rs:6`, one cell each); the case's table has these stages, then Ctrl-B.
const TYPED: [u32; 3] = [5, 5, 5];
/// From that click to the typed link: the comment starts at the click's cell and
/// its `src/lib.rs:6` takes cells 3 to 14 of it, so 9 cells right is in the middle
/// of it, for any cell 8 to 10 px wide.
const LINK_DX: i32 = 76;
/// The linked line, which the cursor is not on after the click: Notes' line 6.
const LINKED: &str = "line 6,";

const OPEN: &str = "kai-call 1\nfunc app:editor/actions@1.0.0#open\nargs {component: \"notes\", path: \"src/lib.rs\", line: 1}\n";
const STATUS: &str = "kai-call 1\nfunc app:editor/actions@1.0.0#status\nargs {}\n";

pub(crate) fn status() -> Result<String, &'static str> {
    ipc::call(NAME, STATUS.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "app-editor: the editor did not answer for its status")
}

/// The cursor, as (line, column), in a status such as `... lib.rs; line 2, column
/// 43; 1 edit(s) ...`.
pub(crate) fn cursor(status: &str) -> Option<(u32, u32)> {
    let rest = &status[status.find("; line ")? + "; line ".len()..];
    let (line, rest) = rest.split_once(", column ")?;
    let column: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some((line.parse().ok()?, column.parse().ok()?))
}

/// Looks at the app's status until `done` likes it. The status is also what makes
/// the app look at a build that is running, as it has no timer of its own.
pub(crate) fn until(
    what: &'static str,
    ms: u64,
    mut done: impl FnMut(&str) -> bool,
) -> Result<String, &'static str> {
    let mut last = String::new();
    for _ in 0..ms / POLL_MS {
        last = status()?;
        if done(&last) {
            return Ok(last);
        }
        threads::sleep_ms(POLL_MS);
    }
    log!(
        "app-editor: timed out waiting for {what}; the editor says {}",
        last.replace('\n', "; ")
    );
    Err(what)
}

/// Whether Notes has an upload the edit service built (`e1`, `e2` ...).
fn built() -> bool {
    components::versions()
        .iter()
        .any(|(name, labels)| name == "notes" && labels.iter().any(|label| label.starts_with('e')))
}

/// Waits for `desktop` to take the screen and the editor and Notes to run.
fn desktop_and_apps() -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        let desktop = process::named("desktop").first().map(|&(pid, _)| pid);
        let shows = desktop.and_then(|pid| {
            process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid))
        });
        if shows.is_some_and(claim::shows)
            && !process::named(NAME).is_empty()
            && !process::named("notes").is_empty()
        {
            break;
        }
        threads::sleep_ms(50);
    }
    if !console::suspended() {
        log!("app-editor: the desktop never took the screen");
        return Err("app-editor: the desktop never took the screen");
    }
    threads::sleep_ms(OPEN_MS);
    Ok(())
}

/// The agent's door: the component's source at a line, shown in the window.
fn open_source() -> Result<(), &'static str> {
    let reply = ipc::call(NAME, OPEN.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "app-editor: the editor did not answer its open")?;
    log!("app-editor: open answered {}", reply.replace('\n', "; "));
    if !reply.contains("Opening notes at src/lib.rs:1") {
        return Err("app-editor: the editor refused to open notes' source");
    }
    until("the editor to show notes' src/lib.rs", WAIT_MS, |said| {
        said.contains("lib.rs") && said.contains("line 1,")
    })?;
    log!("app-editor: the agent opened Notes' src/lib.rs at line 1 in the editor");
    Ok(())
}

/// A click into the code view, which gives it the keys and moves the cursor; the
/// cursor it moved to.
fn click_in() -> Result<(u32, u32), &'static str> {
    let before = cursor(&status()?);
    report(CODE.0 - START.0, CODE.1 - START.1, 0);
    report(0, 0, 1);
    report(0, 0, 0);
    let said = until("the click to move the editor's cursor", WAIT_MS, |said| {
        cursor(said).is_some() && cursor(said) != before
    })?;
    cursor(&said).ok_or("app-editor: the editor's status has no cursor")
}

/// The host types a comment, a few keys a stage, each stage seen in the cursor;
/// then Ctrl-B, and the build's upload arrives in the component store.
fn type_and_build(from: (u32, u32)) -> Result<(), &'static str> {
    let mut column = from.1;
    for keys in TYPED {
        log!("app-editor: ready for keys");
        column += keys;
        until("the typed keys to show in the editor", WAIT_MS, |said| {
            cursor(said) == Some((from.0, column))
        })?;
    }
    log!("app-editor: the host's keys were typed in the editor");
    log!("app-editor: ready for keys");
    let sent = until(
        "the typed edit to reach the edit service",
        WAIT_MS,
        |said| !said.contains("; 0 edit(s)"),
    )?;
    log!("app-editor: after Ctrl-B, {}", sent.replace('\n', "; "));
    for _ in 0..BUILD_MS / POLL_MS {
        if built() {
            log!("app-editor: an edit typed in the editor was built in the background");
            return Ok(());
        }
        // Looking at the status lets the app poll its running build.
        status()?;
        threads::sleep_ms(POLL_MS);
    }
    let last = status()?;
    log!(
        "app-editor: no build arrived; the editor says {}",
        last.replace('\n', "; ")
    );
    Err("app-editor: the typed edit was never built")
}

/// The host holds Ctrl; the click lands on the link in the comment it typed.
fn ctrl_click() -> Result<(), &'static str> {
    if status()?.contains(LINKED) {
        return Err("app-editor: the cursor was on the linked line before the click");
    }
    log!("app-editor: ready for keys");
    threads::sleep_ms(CTRL_LEAD_MS);
    report(LINK_DX, 0, 0);
    report(0, 0, 1);
    report(0, 0, 0);
    until("the Ctrl-click to open the linked line", WAIT_MS, |said| {
        said.contains(LINKED)
    })?;
    log!("app-editor: a Ctrl-click on a source link in the editor opened its line");
    Ok(())
}

pub fn check() -> Result<(), &'static str> {
    desktop_and_apps()?;
    open_source()?;
    let at = click_in()?;
    type_and_build(at)?;
    ctrl_click()
}
