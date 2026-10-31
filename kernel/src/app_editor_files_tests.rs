//! `test=app-editor-files` (P4 task 11, the editor on the default boot): the editor
//! opens a file of the person's own, and a typed edit is saved through the file door
//! and the shell.
//!
//! There is no build service and no edit service: the editor is `desktop`'s window
//! app with the `kai:files` grant, and the shell has the person's files. The case
//! has the shell make `/home/user/todo.txt` (a typed line, as a person would). The
//! host then presses Ctrl-O, types the path and Enter, and the editor reads the file
//! through comphost, `desktop` and the shell and shows it; a typed letter and Ctrl-S
//! write it back the same way, as one logged action. The case reads the file from
//! the shell, then types `undo`, and the file is as it was.
//!
//! Each step of the host's keys (a stage in the case's table, sent when this test
//! logs `ready for keys`) is waited for in the editor's own status, so a slow guest
//! (TCG) only takes longer. Each step shows in comphost's and this test's log lines,
//! which the case in `xtask/src/cases/components.rs` expects.

use crate::app_editor_tests::{NAME, OPEN_MS, WAIT_MS, cursor, status, until};
use crate::display::claim;
use crate::{console, ipc, log, process, threads};
use alloc::string::String;

/// `desktop`, planned before the shell, whose route to it the panel and the file door
/// use; the editor's own line is `kabi::plan::EDITOR`, after the shell.
pub const DESKTOP: &str = "desktop caps=display+input+serve pages=8192 stack_pages=512";

const CALL_MS: u64 = 60_000;
/// The file as the shell makes it: 24 bytes.
const MAKE: &str = "fs_create(\"todo.txt\", \"buy milk\\ncall Ann\\nsleep\\n\");";
const READ: &str = "show(fs_read(\"todo.txt\"));";
const AFTER_EDIT: &str = "zbuy milk";

/// The shell's wire reply to `line`, as a typed line.
fn shell(line: &str) -> Result<String, &'static str> {
    let reply = ipc::call("shell", line.as_bytes(), CALL_MS)
        .map_err(|_| "app-editor-files: the shell did not answer")?;
    Ok(String::from_utf8_lossy(&reply).into_owned())
}

/// Waits for `desktop` to take the screen and the editor to run.
fn desktop_and_editor() -> Result<(), &'static str> {
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
        log!("app-editor-files: the desktop never took the screen");
        return Err("app-editor-files: the desktop never took the screen");
    }
    threads::sleep_ms(OPEN_MS);
    Ok(())
}

/// A typed line that makes the file.
fn make_the_file() -> Result<(), &'static str> {
    let reply = shell(MAKE)?;
    if !reply.starts_with("ok") {
        log!("app-editor-files: the shell answered {reply:?}");
        return Err("app-editor-files: the shell could not make the file");
    }
    log!("app-editor-files: the shell made the file the editor will open");
    Ok(())
}

/// The host types Ctrl-O and the path; the editor shows the file.
fn open_file() -> Result<(), &'static str> {
    // Ctrl-O and the first half of the path, then the rest and Enter: two stages the
    // host sends in order.
    log!("app-editor-files: ready for keys");
    log!("app-editor-files: ready for keys");
    until("the file to open in the editor", WAIT_MS, |said| {
        said.contains("todo.txt") && cursor(said).is_some()
    })?;
    log!("app-editor-files: Ctrl-O opened the file in the editor");
    Ok(())
}

/// One typed letter shows in the cursor; Ctrl-S writes the file and the editor says so.
fn type_and_save() -> Result<(), &'static str> {
    log!("app-editor-files: ready for keys");
    until("the typed letter to show", WAIT_MS, |said| {
        cursor(said).is_some_and(|(_, column)| column == 1)
    })?;
    log!("app-editor-files: a typed letter showed in the file");
    log!("app-editor-files: ready for keys");
    let said = until("the editor to say it saved the file", WAIT_MS, |said| {
        said.contains("saved ")
    })?;
    log!(
        "app-editor-files: after Ctrl-S, {}",
        said.replace('\n', "; ")
    );
    log!("app-editor-files: Ctrl-S saved the file through the shell");
    Ok(())
}

/// The file the shell reads now.
fn disk() -> Result<String, &'static str> {
    shell(READ)
}

/// The file on the disk has the edit; `undo` puts it back as it was.
fn disk_and_undo() -> Result<(), &'static str> {
    let now = disk()?;
    if !now.contains(AFTER_EDIT) {
        log!("app-editor-files: the disk has {now:?}");
        return Err("app-editor-files: the saved file is not on the disk");
    }
    log!("app-editor-files: the file on the disk has the edit");
    let undone = shell("undo")?;
    if !undone.starts_with("ok") {
        log!("app-editor-files: undo answered {undone:?}");
        return Err("app-editor-files: undo did not run");
    }
    let back = disk()?;
    if back.contains(AFTER_EDIT) || !back.contains("buy milk") {
        log!("app-editor-files: after undo the disk has {back:?}");
        return Err("app-editor-files: undo did not put the file back");
    }
    log!("app-editor-files: undo put the file back as it was");
    Ok(())
}

pub fn check() -> Result<(), &'static str> {
    desktop_and_editor()?;
    log!(
        "app-editor-files: the editor says {}",
        status()?.replace('\n', "; ")
    );
    make_the_file()?;
    open_file()?;
    type_and_save()?;
    disk_and_undo()?;
    Ok(())
}
