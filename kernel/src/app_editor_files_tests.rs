//! `test=app-editor-files` (P4 task 11, the editor on the default boot): the editor
//! makes, saves and reopens a file of the person's own through the file door and the
//! shell, and `undo` takes a save back.
//!
//! There is no build service and no edit service: the editor is `desktop`'s window
//! app with the `kai:files` grant, and the shell has the person's files. A typed
//! shell line cannot call the file rows (those are a model script's), so the editor
//! makes the file itself, as a person would:
//!
//! 1. Ctrl-O and the path `/home/user/todo.txt`, which is not there: the editor opens
//!    a new file;
//! 2. four typed letters and Ctrl-S: the file is made (`fs_create`, 4 bytes);
//! 3. one more letter and Ctrl-S: the file is replaced (`fs_write`, 5 bytes);
//! 4. `undo`, typed at the shell: the second save is taken back;
//! 5. Ctrl-O and the path again: the editor reads the file, and comphost's log says it
//!    came back as the 4 bytes of the first save.
//!
//! Each step of the host's keys (a stage in the case's table, sent when this test logs
//! `ready for keys`) is waited for in the editor's own status, so a slow guest (TCG)
//! only takes longer. Each step shows in comphost's and this test's log lines, which the
//! case in `xtask/src/cases/components.rs` expects.

use crate::app_editor_tests::{NAME, OPEN_MS, WAIT_MS, cursor, status, until};
use crate::display::claim;
use crate::{console, ipc, log, process, threads};
use alloc::string::String;

/// `desktop`, planned before the shell, whose route to it the panel and the file door
/// use; the editor's own line is `kabi::plan::EDITOR`, after the shell.
pub const DESKTOP: &str = "desktop caps=display+input+serve pages=8192 stack_pages=512";

const CALL_MS: u64 = 60_000;

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

/// Whether the editor shows a file of its own (its title starts `file disk:`).
fn showing_file(said: &str) -> bool {
    said.starts_with("file disk:") && said.contains("todo.txt")
}

/// Ctrl-O and the path: two stages of the host's keys, sent in order.
fn ask_to_open() {
    log!("app-editor-files: ready for keys");
    log!("app-editor-files: ready for keys");
}

/// A path that is not there opens a new file.
fn open_new() -> Result<(), &'static str> {
    ask_to_open();
    until("a new file to open in the editor", WAIT_MS, |said| {
        showing_file(said) && cursor(said) == Some((1, 0))
    })?;
    log!("app-editor-files: Ctrl-O on a path that is not there opened a new file");
    Ok(())
}

/// Typed letters, then Ctrl-S; the editor says how many bytes it saved.
fn type_and_save(column: u32, bytes: &str, what: &'static str) -> Result<(), &'static str> {
    log!("app-editor-files: ready for keys");
    until("the typed letters to show", WAIT_MS, |said| {
        cursor(said) == Some((1, column))
    })?;
    log!("app-editor-files: ready for keys");
    let said = until("the editor to say it saved the file", WAIT_MS, |said| {
        said.contains(&alloc::format!("saved /home/user/todo.txt ({bytes} bytes)"))
    })?;
    log!("app-editor-files: {what}: {}", said.replace('\n', "; "));
    Ok(())
}

/// `undo` takes the last save back, and the editor reads the file again.
fn undo_and_reopen() -> Result<(), &'static str> {
    let undone = shell("undo")?;
    if !undone.starts_with("ok") {
        log!("app-editor-files: undo answered {undone:?}");
        return Err("app-editor-files: undo did not run");
    }
    log!("app-editor-files: undo took the second save back");
    ask_to_open();
    // The new screen starts at line 1, column 0; the old one was at the end of the text.
    until("the file to be read again", WAIT_MS, |said| {
        showing_file(said) && cursor(said) == Some((1, 0))
    })?;
    log!("app-editor-files: the file read again is the first save's");
    Ok(())
}

pub fn check() -> Result<(), &'static str> {
    desktop_and_editor()?;
    log!(
        "app-editor-files: the editor says {}",
        status()?.replace('\n', "; ")
    );
    open_new()?;
    type_and_save(4, "4", "Ctrl-S made the file")?;
    type_and_save(5, "5", "Ctrl-S replaced the file")?;
    undo_and_reopen()
}
