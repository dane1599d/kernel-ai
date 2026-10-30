//! `test=app-route` (P4, the agent-to-app route): the model's `notes_add` call
//! reaches the notes app through the supervisor's `app-call` (APP_CALL), the
//! shell having loaded notes' rows on the call, and `undo` puts notes' state
//! back with its own `kai-undo 1`. The kernel counts the notes before and after.

use crate::ipc;
use crate::log;
use crate::shell_tests::Expect::StartsWith;
use crate::shell_tests::expect_lines;
use alloc::string::String;

const COUNT: &str = "kai-call 1\nfunc app:notes/actions@1.0.0#count\nargs {}\n";
const CALL_MS: u64 = 60_000;

fn count() -> Result<String, &'static str> {
    ipc::call("notes", COUNT.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "app-route: notes did not answer")
}

pub fn check() -> Result<(), &'static str> {
    crate::live_tests::wait_for_host(0).map_err(|_| "app-route: notes never started")?;
    expect_lines(&[("add a note saying buy milk", StartsWith("ok "))])?;
    let added = count()?;
    log!(
        "app-route: after the call, notes says {}",
        added.replace('\n', "; ")
    );
    if !added.contains("notes: 1,") {
        return Err("app-route: the note was not added");
    }
    expect_lines(&[("undo", StartsWith("ok Undone: notes as it was"))])?;
    if !count()?.contains("notes: 0,") {
        return Err("app-route: undo did not put notes back");
    }
    log!(
        "app-route: the agent's notes_add reached notes through the supervisor, and undo put it back"
    );
    Ok(())
}
