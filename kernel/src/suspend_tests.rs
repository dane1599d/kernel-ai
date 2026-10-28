//! `test=app-suspend` (P3 task 11): an app suspended and resumed through its
//! host. `notes` (version 2, `kai:live`) holds a note; `ctl suspend` saves it
//! and drops the instance, whose linear memory is a MEM_RESERVE reservation of
//! its own, released with it: the kernel's free frames rise by at least the
//! guest's committed memory (a Rust guest starts with 1 MiB and more). `ctl
//! resume` brings it back from the saved state, committing again, and the
//! note is still there. comphost's own heap figures ride in each reply too.

use crate::{ipc, log, memory};
use alloc::string::String;

const NAME: &str = "notes";
const CALL_MS: u64 = 60_000;
/// Least the suspend must give back to the kernel: 1 MiB of 4 KiB frames.
const MIN_FREED_FRAMES: u64 = 256;

fn call(request: &str) -> Result<String, &'static str> {
    ipc::call(NAME, request.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "app-suspend: notes did not reply")
}

fn free_frames() -> u64 {
    memory::with(|m| m.free_frames())
}

pub fn check() -> Result<(), &'static str> {
    crate::live_tests::wait_for_host(0).map_err(|_| "app-suspend: notes never started")?;
    let add = "kai-call 1\nfunc app:notes/actions@1.0.0#add\nargs {text: \"buy milk\"}\n";
    if !call(add)?.contains("ok(1)") {
        return Err("app-suspend: the note was not added");
    }
    let before = free_frames();
    let suspended = call("ctl suspend")?;
    let after = free_frames();
    log!("app-suspend: {suspended}");
    if !suspended.starts_with("ok suspended") {
        return Err("app-suspend: notes did not suspend");
    }
    let freed = after.saturating_sub(before);
    log!("app-suspend: free frames {before} before the suspend, {after} after ({freed} back)");
    if freed < MIN_FREED_FRAMES {
        return Err("app-suspend: the suspend gave too few frames back");
    }
    let resumed = call("ctl resume")?;
    let again = free_frames();
    log!("app-suspend: {resumed}");
    if !resumed.starts_with("ok resumed") {
        return Err("app-suspend: notes did not resume");
    }
    if again + MIN_FREED_FRAMES > after {
        log!("app-suspend: free frames {again} after the resume");
        return Err("app-suspend: the resume took no frames");
    }
    let count = "kai-call 1\nfunc app:notes/actions@1.0.0#count\nargs {}\n";
    if !call(count)?.contains("{notes: 1, words: 2}") {
        return Err("app-suspend: the note did not come back");
    }
    log!(
        "app-suspend: notes gave {freed} frames back to the kernel while suspended and came back with its note"
    );
    Ok(())
}
