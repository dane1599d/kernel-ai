//! `test=desk-nap-busy` (the founder's second test, backlog item 1): a window
//! in use never naps. comphost runs `notes-window` with a 3-second nap, as
//! `desk-nap` does. A click gives its window the focus, and a key goes to it
//! every second for ten seconds, more than three naps' time: the app keeps
//! its memory and answers at once. Then a click on the panel takes the focus
//! away, `desktop` tells comphost so (`focus_out`), and the app, unused now,
//! naps and gives its memory back. Before this the Terminal napped under the
//! founder's typing: comphost took no key as use and never heard a window
//! lose the focus.

use crate::display::claim;
use crate::{console, ipc, log, memory, process, threads};
use alloc::string::String;

const NAME: &str = "notes-window";
const WAIT_MS: u64 = 60_000;
const CALL_MS: u64 = 60_000;
/// Time for comphost to start the app and paint its first frame under TCG.
const OPEN_MS: u64 = 10_000;
/// Keys, one a second: over three of the 3-second naps.
const KEYS: u64 = 10;
/// The nap, and time for comphost to notice it is due.
const NAP_MS: u64 = 3_000 + 4_000;
/// Least a nap gives back: the guest's 128 KiB of starting memory.
const MIN_FRAMES: u64 = 32;
/// Where `desktop` starts the pointer: the middle of 1280x800.
const START: (i32, i32) = (640, 400);
/// The panel along the bottom of 1280x800 (36 px tall).
const PANEL: (i32, i32) = (640, 800 - 18);
/// The middle of Notes' content: its 642x454 frame at 48,52, under a 34 px bar.
const WINDOW: (i32, i32) = (48 + 321, 52 + 34 + 210);
/// `a`, down and up (scan code set 1).
const KEY: [u8; 2] = [0x1E, 0x9E];

fn call(request: &str) -> Result<String, &'static str> {
    ipc::call(NAME, request.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "desk-nap-busy: notes-window did not reply")
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
        return Err("desk-nap-busy: the desktop never took the screen");
    }
    threads::sleep_ms(OPEN_MS);
    let add = "kai-call 1\nfunc app:notes/actions@1.0.0#add\nargs {text: \"buy milk\"}\n";
    if !call(add)?.contains("ok(1)") {
        return Err("desk-nap-busy: the note was not added");
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
    threads::sleep_ms(500);
    let awake = free_frames();
    for _ in 0..KEYS {
        crate::claim_tests::type_keys(&KEY);
        threads::sleep_ms(1_000);
    }
    let typing = free_frames();
    log!("desk-nap-busy: free frames {awake} before the keys, {typing} after");
    if typing >= awake + MIN_FRAMES {
        return Err("desk-nap-busy: the window napped while a person typed in it");
    }
    let count = "kai-call 1\nfunc app:notes/actions@1.0.0#count\nargs {}\n";
    if !call(count)?.contains("notes: 1") {
        return Err("desk-nap-busy: the app did not answer after the keys");
    }
    log!("desk-nap-busy: ten seconds of keys in its window kept the app awake");
    // The focus goes to the panel: the window is no longer in use.
    click(PANEL);
    let before = free_frames();
    threads::sleep_ms(NAP_MS);
    let napping = free_frames();
    log!("desk-nap-busy: free frames {before} as the focus left, {napping} after");
    if napping < before + MIN_FRAMES {
        return Err("desk-nap-busy: the window that lost the focus never napped");
    }
    log!("desk-nap-busy: a window in use never napped, and napped once the focus left it");
    Ok(())
}
