//! `test=app-suspend` (P3 task 11): an app suspended and resumed through its
//! host. `notes` (version 2, `kai:live`) holds a note; `ctl suspend` saves it
//! and drops the instance, and comphost's heap in use falls by at least the
//! instance's linear memory, its own region at the cap (`memory_kib`, 2 MiB by
//! default); `ctl resume` brings it back from the saved state, its heap in use
//! rising again, and the note is still there. The heap figures are comphost's
//! own (`user::heap_bytes`), in each reply: its heap only grows, so until lane
//! K's MEM_RELEASE the pages stay with the process.

use crate::{ipc, log};
use alloc::string::String;

const NAME: &str = "notes";
const CALL_MS: u64 = 60_000;
/// Least the suspend must free: the instance's memory region, at the default
/// cap.
const MIN_FREED_KIB: u64 = 2048;

fn call(request: &str) -> Result<String, &'static str> {
    ipc::call(NAME, request.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "app-suspend: notes did not reply")
}

/// The `heap A KiB -> B KiB` comphost adds to a suspend's or resume's reply.
fn heap(reply: &str) -> Option<(u64, u64)> {
    let (_, figures) = reply.rsplit_once("; heap ")?;
    let (before, after) = figures.split_once(" KiB -> ")?;
    let after = after.strip_suffix(" KiB")?;
    Some((before.parse().ok()?, after.parse().ok()?))
}

pub fn check() -> Result<(), &'static str> {
    crate::live_tests::wait_for_host(0).map_err(|_| "app-suspend: notes never started")?;
    let add = "kai-call 1\nfunc app:notes/actions@1.0.0#add\nargs {text: \"buy milk\"}\n";
    if !call(add)?.contains("ok(1)") {
        return Err("app-suspend: the note was not added");
    }
    let suspended = call("ctl suspend")?;
    log!("app-suspend: {suspended}");
    let Some((before, after)) = heap(&suspended).filter(|_| suspended.starts_with("ok")) else {
        return Err("app-suspend: notes did not suspend");
    };
    let freed = before.saturating_sub(after);
    if freed < MIN_FREED_KIB {
        log!("app-suspend: only {freed} KiB came back");
        return Err("app-suspend: the suspend freed too little");
    }
    let resumed = call("ctl resume")?;
    log!("app-suspend: {resumed}");
    if !resumed.starts_with("ok resumed") {
        return Err("app-suspend: notes did not resume");
    }
    match heap(&resumed) {
        Some((before, after)) if after >= before + MIN_FREED_KIB => {}
        _ => return Err("app-suspend: the resume took no memory back"),
    }
    let count = "kai-call 1\nfunc app:notes/actions@1.0.0#count\nargs {}\n";
    if !call(count)?.contains("{notes: 1, words: 2}") {
        return Err("app-suspend: the note did not come back");
    }
    log!("app-suspend: notes freed {freed} KiB while suspended and came back with its note");
    Ok(())
}
