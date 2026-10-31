//! `test=desk-tour`: a screenshot tour of the desktop and its apps, for people
//! to look at, not a gate. Each step sets the screen up, logs "ready for keys",
//! and the host takes a QEMU `screendump` (`xtask/src/cases/tour.rs`); the
//! steps drive the system as a person and the mock model would: lines to the
//! shell, keys through the i8042, and clicks through the pointer queue. A step
//! that does not come out as planned is logged and the tour goes on, so the
//! pictures show what is really there.

use crate::agent_desk_tests::DESK;
use crate::claim_tests::type_keys;
use crate::display::claim;
use crate::{ask_box, ipc, log, process, terminal, threads};
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

const CASE: &str = "desk-tour";
/// The shell with a route to `desktop`, `cc` for its prompt, and `files` and
/// `state` on the case's data disk: the theme and the files are saved there.
pub const SHELL: &str = "shell caps=serve+call+policy+stop+script+files+state uses=llm+supervisor+cc+desktop pages=4096 stack_pages=512 burst_ms=5000";
/// Every app the image packs, each started when it is first opened. The
/// System monitor's routes are `desktop` then `llm` (`comphost/system.rs`).
pub const APPS: &str = "notes-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1 start=demand\n\
     files-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1 start=demand\n\
     terminal-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1 start=demand\n\
     settings-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1 start=demand\n\
     monitor-window program=comphost caps=serve+component+call uses=desktop+llm pages=8192 stack_pages=512 burst_ms=20000 version=1 start=demand";
/// `desktop`, planned before the shell, whose route it is.
pub const DESKTOP: &str = DESK;
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
const SHIFT: u8 = 0x2A;

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

/// A line to the shell, as the console sends it; its reply is logged.
fn line(text: &str) {
    match ipc::call("shell", text.as_bytes(), CALL_MS) {
        Ok(reply) => {
            let reply = String::from_utf8_lossy(&reply);
            let first = reply.lines().next().unwrap_or("");
            log!("{CASE}: {text:?} -> {first}");
        }
        Err(_) => log!("{CASE}: {text:?}: the shell did not reply"),
    }
}

/// Opens `app` with `ui_open` and waits for its process and first frames.
fn open(app: &str) {
    line(&alloc::format!("ui_open(\"{app}\")"));
    let service = alloc::format!("{app}-window");
    if wait("the app to start", || !process::named(&service).is_empty()) {
        threads::sleep_ms(OPEN_MS);
    }
}

/// Set-1 make code of `c`, and whether it needs Shift (a US layout).
fn scan(c: char) -> Option<(u8, bool)> {
    const LETTERS: &[u8; 26] = &[
        0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18,
        0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
    ];
    Some(match c {
        'a'..='z' => (LETTERS[c as usize - 'a' as usize], false),
        '1'..='9' => (0x02 + (c as u8 - b'1'), false),
        '0' => (0x0B, false),
        ' ' => (0x39, false),
        '\n' => (0x1C, false),
        '-' => (0x0C, false),
        '=' => (0x0D, false),
        '+' => (0x0D, true),
        '*' => (0x09, true),
        '.' => (0x34, false),
        _ => return None,
    })
}

/// Types `text` as a keyboard would.
fn type_text(text: &str) {
    let mut bytes = Vec::new();
    for c in text.chars() {
        let Some((code, shift)) = scan(c) else {
            log!("{CASE}: no key for {c:?}");
            continue;
        };
        if shift {
            bytes.push(SHIFT);
        }
        bytes.extend([code, code | 0x80]);
        if shift {
            bytes.push(SHIFT | 0x80);
        }
    }
    for pair in bytes.chunks(8) {
        type_keys(pair);
        threads::sleep_ms(40);
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
    line("appearance_set(\"reduced_motion\", \"on\")");
    for (name, text) in [
        ("plan.txt", "the plan"),
        ("ideas.txt", "ideas"),
        ("shopping.txt", "milk"),
    ] {
        line(&alloc::format!("fs_create(\"{name}\", \"{text}\")"));
    }
    shot("01-desktop");

    // The Apps sheet: `apps` sent in the panel.
    type_keys(&CTRL_SPACE);
    threads::sleep_ms(SETTLE_MS);
    type_text("apps\n");
    shot("02-apps");
    // Esc closes the sheet and leaves the panel.
    type_keys(&[0x01, 0x81]);

    open("files");
    shot("03-files");

    open("terminal");
    type_text("1 + 2\n");
    threads::sleep_ms(SETTLE_MS);
    type_text("6 * 7\n");
    shot("04-terminal");

    open("notes");
    // Kai adds the note from the panel, as a person would ask.
    type_keys(&CTRL_SPACE);
    threads::sleep_ms(SETTLE_MS);
    type_text("add a note saying buy milk\n");
    threads::sleep_ms(SETTLE_MS);
    shot("05-notes");

    open("settings");
    line("settings_open(\"appearance\")");
    shot("06-settings-appearance");
    line("settings_open(\"access\")");
    shot("07-settings-access");

    open("monitor");
    shot("08-monitor");

    // Kai acting: the panel's done state from Notes' step is behind; an ask
    // shows on the card, and the person says no.
    threads::spawn("asker", asker, 0)?;
    // Every step takes its picture, card or not: the host names them in order.
    let shown = wait("the ask on the card", ask_box::shown);
    shot("09-kai-asks");
    if shown {
        type_keys(&NO);
        wait("the shell's answer", || DONE.load(Ordering::Acquire));
    }
    if !ASKED.load(Ordering::Acquire) {
        log!("{CASE}: the model's act did not ask");
    }
    shot("10-kai-done");

    line("appearance_set(\"theme\", \"light\")");
    shot("11-light");

    // Overlapping windows, Files brought to the front and focused.
    line("ui_focus(\"files\")");
    shot("12-overlap");

    log!("{CASE}: the tour is done");
    Ok(())
}
