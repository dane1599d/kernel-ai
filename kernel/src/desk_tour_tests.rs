//! `test=desk-tour`: a screenshot tour of Kai Desk and its apps, for people
//! to look at, not a gate; xtask boots it at 1280x800, 1024x768 and
//! 1920x1080, and the steps are the same at each size. Each step sets the screen up, logs "ready for keys",
//! and the host takes a QEMU `screendump` (`xtask/src/cases/tour.rs`); the
//! steps drive the system as a person and the mock model would: lines to the
//! shell, keys through the i8042, and clicks through the pointer queue. A step
//! that does not come out as planned is logged and the tour goes on, so the
//! pictures show what is really there.

use crate::claim_tests::{type_keys, type_text};
use crate::display::claim;
use crate::{allow, ask_box, ipc, log, process, terminal, threads};
use alloc::string::String;
use core::sync::atomic::{AtomicBool, Ordering};
use termcore::menu::{self, Answer};

// The tour boots the v0.1 system as the default boot plans it
// (`kabi::plan::system`, no plan line of its own) on a disk with `kai-data`,
// where the theme and the files are saved: its pictures are the real boot's.
const CASE: &str = "desk-tour";
const WAIT_MS: u64 = 60_000;
const CALL_MS: u64 = 30_000;
/// Time for comphost to start an app and paint its first frames.
const OPEN_MS: u64 = 6_000;
/// Time for a typed line or a click to show.
const SETTLE_MS: u64 = 2_000;
/// Time for the host's screendump once the step says it is ready.
const SHOT_MS: u64 = 3_000;
/// Ctrl down, Space down and up, Ctrl up: the panel takes the keys.
const CTRL_SPACE: [u8; 4] = [0x1D, 0x39, 0xB9, 0x9D];
/// `n` down and up: no, on the card.
const NO: [u8; 2] = [0x31, 0xB1];
/// A Super tap: Left Super down and up, which opens Start.
const SUPER: [u8; 4] = [0xE0, 0x5B, 0xE0, 0xDB];
/// Esc down and up.
const ESC: [u8; 2] = [0x01, 0x81];

static ASKED: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);

fn wait(what: &str, mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..WAIT_MS / 50 {
        if done() {
            return true;
        }
        threads::sleep_ms(50);
    }
    log!("{CASE}: timed out waiting for {what}");
    false
}

/// The host takes screenshot `name` now.
fn shot(name: &str) {
    threads::sleep_ms(SETTLE_MS);
    log!("{CASE}: {name}: ready for keys");
    threads::sleep_ms(SHOT_MS);
}

/// A line to the shell, as the console sends it; its reply is logged. A row
/// runs only from the model's script, so each step is a request the mock
/// model answers (`xtask/src/mock_llm.rs`). A step the shell asks about is
/// allowed through the trusted path, as `app-terminal` allows its line.
fn line(text: &str) {
    let Some(reply) = say(text) else {
        return;
    };
    let Ok(asked) = menu::Menu::decode(&reply) else {
        return;
    };
    let Some(allowed) = allow::allow(asked.id, asked.call(), Answer::Allow)
        .ok()
        .flatten()
    else {
        log!("{CASE}: {text:?}: the trusted path could not allow it");
        return;
    };
    say(&menu::granted_line(
        asked.id,
        Answer::Allow,
        allowed.granted,
    ));
    allow::end(&allowed);
}

/// One line to the shell; its reply, whose first line is logged.
fn say(text: &str) -> Option<String> {
    match ipc::call("shell", text.as_bytes(), CALL_MS) {
        Ok(reply) => {
            let reply = String::from_utf8_lossy(&reply).into_owned();
            let first = reply.lines().next().unwrap_or("");
            log!("{CASE}: {text:?} -> {first}");
            Some(reply)
        }
        Err(_) => {
            log!("{CASE}: {text:?}: the shell did not reply");
            None
        }
    }
}

/// Asks for `app`'s window (`request`, which the model answers with
/// `ui_open`) and waits for its process and first frames.
fn open(app: &str, request: &str) {
    line(request);
    let service = alloc::format!("{app}-window");
    if wait("the app to start", || !process::named(&service).is_empty()) {
        threads::sleep_ms(OPEN_MS);
    }
}

/// Asks the model something that ends in an ask on `desktop`'s card.
fn asker(_: usize) {
    ASKED.store(
        terminal::shell_line_answered("look at settings"),
        Ordering::Release,
    );
    DONE.store(true, Ordering::Release);
}

pub fn check() -> Result<(), &'static str> {
    if !wait("desktop and the shell", || {
        claim::showing() && ask_box::alive() && !process::named("shell").is_empty()
    }) {
        return Err("desk-tour: the desktop never came up");
    }
    // Pictures, not animations: reduced motion, and files for Files to show.
    for request in [
        "turn on reduced motion",
        "create notes.txt saying hello",
        "create plan.txt saying the plan",
        "create ideas.txt saying ideas",
    ] {
        line(request);
    }
    shot("01-desktop");

    // Start: a Super tap opens it on its pins, with its box focused.
    type_keys(&SUPER);
    shot("02-start");
    // Typing searches: the apps that match, and Ask Kai.
    type_text("no");
    shot("03-search");
    // Esc closes Start. `apps` sent in the pane opens it on All apps.
    type_keys(&ESC);
    type_keys(&CTRL_SPACE);
    threads::sleep_ms(SETTLE_MS);
    type_text("apps\n");
    shot("04-all-apps");
    // Esc closes Start, and Esc in the pane's idle input closes the pane.
    type_keys(&ESC);
    threads::sleep_ms(SETTLE_MS);
    type_keys(&ESC);

    open("files", "open the files app");
    shot("05-files");

    open("terminal", "open the terminal");
    type_text("1 + 2\n");
    threads::sleep_ms(SETTLE_MS);
    type_text("6 * 7\n");
    shot("06-terminal");

    open("notes", "open notes");
    // Kai adds the note from the panel, as a person would ask.
    type_keys(&CTRL_SPACE);
    threads::sleep_ms(SETTLE_MS);
    type_text("add a note saying buy milk\n");
    threads::sleep_ms(SETTLE_MS);
    shot("07-notes");

    open("settings", "open settings");
    line("show the appearance settings");
    shot("08-settings-appearance");
    line("show the access settings");
    shot("09-settings-access");

    open("monitor", "open the monitor");
    shot("10-monitor");

    // A line in the pane: Kai's answer in words in its history (the mock
    // model answers "hello" with prose and no script).
    type_keys(&CTRL_SPACE);
    threads::sleep_ms(SETTLE_MS);
    type_text("hello\n");
    shot("11-kai-pane");

    // Kai acting: the panel's done state from Notes' step is behind; an ask
    // shows on the card, and the person says no.
    threads::spawn("asker", asker, 0)?;
    // Every step takes its picture, card or not: the host names them in order.
    let shown = wait("the ask on the card", ask_box::shown);
    shot("12-kai-asks");
    if shown {
        // Past the card's settle time, in which it takes no key (#864).
        threads::sleep_ms(termcore::menu::SETTLE_MS + 300);
        type_keys(&NO);
        wait("the shell's answer", || DONE.load(Ordering::Acquire));
    }
    if !ASKED.load(Ordering::Acquire) {
        log!("{CASE}: the model's act did not ask");
    }
    shot("13-kai-done");

    line("use the light theme");
    shot("14-light");

    // Overlapping windows, Files brought to the front and focused.
    line("switch to files");
    shot("15-overlap");

    log!("{CASE}: the tour is done");
    Ok(())
}
