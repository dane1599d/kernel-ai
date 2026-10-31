//! `test=desk-nap` (P3 task 12b): an unused app naps and a click wakes it.
//! comphost runs `notes-window` with a 3-second nap (the default is 2
//! minutes). The case adds a note, then leaves the app alone: comphost
//! suspends it, and the kernel's free frames rise as its memory goes back
//! (MEM_RELEASE). A click on the panel and then on the app's window moves the
//! focus to it; comphost hears `focus_in` and wakes the app, which takes
//! memory again and still has its note.

use crate::display::claim;
use crate::{console, ipc, log, memory, process, threads};
use alloc::string::String;

const NAME: &str = "notes-window";
/// The plan lines this case adds.
pub const PLAN: &str = "desktop caps=display+input+serve pages=8192 stack_pages=512\n\
     notes-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1 nap_ms=3000";
const WAIT_MS: u64 = 60_000;
const CALL_MS: u64 = 60_000;
/// Time for comphost to start the app and paint its first frame under TCG.
const OPEN_MS: u64 = 10_000;
/// The nap, and time for comphost to notice it is due.
const NAP_MS: u64 = 3_000 + 4_000;
/// Least the nap must give back: the guest's 128 KiB of starting memory.
const MIN_FRAMES: u64 = 32;
/// Where `desktop` starts the pointer: the middle of 1280x800.
const START: (i32, i32) = (640, 400);
/// The panel along the bottom of 1280x800 (36 px tall).
const PANEL: (i32, i32) = (640, 800 - 18);
/// The middle of Notes' content: its 642x454 frame at 48,52, under a 34 px bar.
const WINDOW: (i32, i32) = (48 + 321, 52 + 34 + 210);

fn call(request: &str) -> Result<String, &'static str> {
    ipc::call(NAME, request.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "desk-nap: notes-window did not reply")
}

fn free_frames() -> u64 {
    memory::with(|m| m.free_frames())
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
        return Err("desk-nap: the desktop never took the screen");
    }
    threads::sleep_ms(OPEN_MS);
    let add = "kai-call 1\nfunc app:notes/actions@1.0.0#add\nargs {text: \"buy milk\"}\n";
    if !call(add)?.contains("ok(1)") {
        return Err("desk-nap: the note was not added");
    }
    let awake = free_frames();
    threads::sleep_ms(NAP_MS);
    let napping = free_frames();
    log!("desk-nap: free frames {awake} awake, {napping} napping");
    if napping < awake + MIN_FRAMES {
        return Err("desk-nap: the unused app gave no memory back");
    }
    let mut at = START;
    let mut click = |to: (i32, i32)| {
        crate::ui_tests::report(to.0 - at.0, to.1 - at.1, 0);
        crate::ui_tests::report(0, 0, 1);
        crate::ui_tests::report(0, 0, 0);
        at = to;
    };
    click(PANEL);
    click(WINDOW);
    threads::sleep_ms(2_000);
    let woken = free_frames();
    log!("desk-nap: free frames {woken} after the click");
    if woken + MIN_FRAMES > napping {
        return Err("desk-nap: the click did not wake the app");
    }
    let count = "kai-call 1\nfunc app:notes/actions@1.0.0#count\nargs {}\n";
    if !call(count)?.contains("{notes: 1, words: 2}") {
        return Err("desk-nap: the note did not survive the nap");
    }
    log!("desk-nap: Notes napped unused, gave its memory back, and a click woke it with its note");
    Ok(())
}
