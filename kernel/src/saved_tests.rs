//! `test=app-saved` (P4, app storage that survives a reboot): `notes-window`
//! runs with `dir=apps`, so comphost keeps each committed checkpoint in
//! `/system/apps/notes-window/`. Boot 1 adds a note; boot 2 (`reread`), on the
//! same disk, starts with an empty kernel ledger, and comphost restores the app
//! from its saved state, so the note is still there.

use crate::{ipc, log};
use alloc::format;
use alloc::string::String;

const NAME: &str = "notes-window";
const CALL_MS: u64 = 60_000;
const START_MS: u64 = 120_000;

/// The plan lines: `fs`, and the app with its folder.
pub fn plan() -> String {
    format!(
        "{}\nnotes-window program=comphost caps=serve+component dir=apps pages=8192 stack_pages=512 burst_ms=20000 version=1",
        kabi::plan::FS
    )
}

fn call(request: &str) -> Result<String, &'static str> {
    ipc::call(NAME, request.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "app-saved: notes-window did not reply")
}

pub fn check(reread: bool) -> Result<(), &'static str> {
    if !(0..START_MS / 10).any(|_| {
        let up = ipc::server_of(NAME).is_some();
        if !up {
            crate::threads::sleep_ms(10);
        }
        up
    }) {
        return Err("app-saved: notes-window never started");
    }
    let count = "kai-call 1\nfunc app:notes/actions@1.0.0#count\nargs {}\n";
    if reread {
        let tally = call(count)?;
        log!("app-saved: boot 2 counts {tally}");
        if !tally.contains("{notes: 1, words: 2}") {
            return Err("app-saved: the note did not survive the reboot");
        }
        log!("app-saved: boot 2 found boot 1's note, restored from the app's saved state");
        return Ok(());
    }
    let add = "kai-call 1\nfunc app:notes/actions@1.0.0#add\nargs {text: \"buy milk\"}\n";
    if !call(add)?.contains("ok(1)") {
        return Err("app-saved: the note was not added");
    }
    log!("app-saved: boot 1 added a note");
    Ok(())
}
