//! `test=founder-walk` (founder test 2, backlog test 2): the founder's
//! VirtualBox walk of 2026-11-14, end to end, on the v0.1 system as the default
//! boot plans it (`kabi::plan::system`) with the mock model, at the size the
//! case's `display=` sets. Everything goes in as a person's keys and clicks,
//! through the i8042 and the pointer queue:
//! 1. `open terminal` in the panel opens the Terminal, which takes the focus.
//!    "show me the cpus please" typed into it shows in its input row (the
//!    screen's own pixels change), and Enter runs it: the Terminal shows
//!    "CPUs:", and the host checks that the shell ran exactly that line.
//! 2. "what can you do with the cpus" is answered from the rows. The model's
//!    clarify for "do more with the cpus" offers only the option a row backs.
//! 3. The next line is a request of its own (the host checks the shell's
//!    note), and it asks first: `y` twice on the trusted card, and its file is
//!    in `fs`. The same from the panel, whose bar the host reads (`log_bar=1`).
//! 4. "add a note saying hello dane" in the panel reaches Notes, which nothing
//!    had started.
//! 5. `open notes` shows its window, its close control closes it, and `open
//!    notes` brings it back.

use crate::claim_tests::{type_keys, type_text};
use crate::desk_tests::{self, report};
use crate::display::claim;
use crate::{ask_box, display, ipc, log, process, threads};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const CASE: &str = "founder-walk";
const CALL_MS: u64 = 60_000;
/// Time for comphost to start an app and paint its first frames under TCG.
const OPEN_MS: u64 = 10_000;
/// Time for a typed line or a click to show.
const SETTLE_MS: u64 = 2_000;
/// How long a line may take to show its answer, a model's rounds included.
const REPLY_MS: u64 = 60_000;
/// How often an answer is looked for.
const POLL_MS: u64 = 1_000;
/// Time for a key on the card to reach `desktop`.
const KEY_MS: u64 = 1_000;
/// Ctrl down, Space down and up, Ctrl up: the panel takes the keys.
const CTRL_SPACE: [u8; 4] = [0x1D, 0x39, 0xB9, 0x9D];
const ENTER: [u8; 2] = [0x1C, 0x9C];
/// `y` down and up: on the card, once arms Run and once more answers it.
const YES: [u8; 2] = [0x15, 0x95];

const TERMINAL: &str = "terminal-window";
const NOTES: &str = "notes-window";
const READ_ROWS: &str =
    "kai-call 1\nfunc app:terminal-window/actions@1.0.0#read\nargs {since: 0}\n";
const COUNT_NOTES: &str = "kai-call 1\nfunc app:notes/actions@1.0.0#count\nargs {}\n";

/// The lines the walk types, which the mock model answers
/// (`xtask/src/mock_llm.rs`), and what their answers show.
const CPUS_LINE: &str = "show me the cpus please";
const ABILITIES_LINE: &str = "what can you do with the cpus";
const CLARIFY_LINE: &str = "do more with the cpus";
/// The model's clarify options: one a row backs, and one none does.
const BACKED: &str = "show the cpus";
const UNBACKED: &str = "increase CPU frequency";
/// Lines that name the change but not its file, so each asks first.
const TERMINAL_ASK: &str = "save a note for the terminal";
const TERMINAL_FILE: &str = "walk-term.txt";
const PANEL_ASK: &str = "save a note for the walk";
const PANEL_FILE: &str = "walk-panel.txt";
/// What the model writes in both files.
const FILE_TEXT: &str = "hello from the walk";
const NOTE_LINE: &str = "add a note saying hello dane";

/// The side of a square of the screen [`signature`] hashes as one.
const TILE: usize = 16;
/// The fewest squares of one row of them that typing the line must change:
/// its text spans a dozen or more, a blinking caret one or two.
const MIN_TYPED_TILES: usize = 6;
/// Tries at a still screen before the typing, [`SETTLE_MS`] apart.
const STILL_TRIES: usize = 10;

/// Notes' close control, its window the desk's second: the cascade's first
/// frame is at 48,52 under the 28-px top bar, the next 32 px on each way, and
/// Notes' frame is 642 px wide, its control 17 px in from the top right
/// (`ui-window-show` finds it so).
const NOTES_CLOSE: (i32, i32) = (48 + 32 + 642 - 17, 52 + 32 + 17);
/// Farther than any screen is wide: `desktop` stops the pointer at the corner.
const TO_CORNER: i32 = -4_000;

/// A FNV-1a hash of each [`TILE`]-pixel square of the screen, row by row, and
/// how many squares make a row.
fn signature() -> Result<(Vec<u64>, usize), &'static str> {
    let surface = display::surface().ok_or("founder-walk: no display")?;
    let across = surface.width.div_ceil(TILE);
    let mut tiles = alloc::vec![0xcbf2_9ce4_8422_2325u64; across * surface.height.div_ceil(TILE)];
    for y in 0..surface.height {
        for x in 0..surface.width {
            // SAFETY: the surface is `stride * height` pixels from `base`, and
            // (x, y) is inside it; a read races only `desktop`'s writes.
            let pixel = unsafe {
                surface
                    .base
                    .as_ptr()
                    .add(y * surface.stride + x)
                    .read_volatile()
            };
            let tile = &mut tiles[(y / TILE) * across + x / TILE];
            *tile = (*tile ^ u64::from(pixel)).wrapping_mul(0x100_0000_01b3);
        }
    }
    Ok((tiles, across))
}

/// The most squares that differ between two signatures in one row of them.
fn most_changed_in_a_row(before: &[u64], after: &[u64], across: usize) -> usize {
    before
        .chunks(across)
        .zip(after.chunks(across))
        .map(|(was, now)| was.iter().zip(now).filter(|(a, b)| a != b).count())
        .max()
        .unwrap_or(0)
}

/// The screen once it has stopped changing more than a caret does.
fn still_screen() -> Result<(Vec<u64>, usize), &'static str> {
    let (mut before, across) = signature()?;
    for _ in 0..STILL_TRIES {
        threads::sleep_ms(SETTLE_MS);
        let (now, _) = signature()?;
        let changed = most_changed_in_a_row(&before, &now, across);
        before = now;
        if changed < MIN_TYPED_TILES {
            return Ok((before, across));
        }
    }
    Err("founder-walk: the screen never kept still before the typing")
}

/// `text` sent from the panel, as a person types it there.
fn panel_line(text: &str) {
    type_keys(&CTRL_SPACE);
    threads::sleep_ms(SETTLE_MS);
    type_text(text);
    type_keys(&ENTER);
}

/// `text` typed into the focused window, and Enter.
fn window_line(text: &str) {
    type_text(text);
    type_keys(&ENTER);
}

/// The Terminal's rows, as its `read` action gives them.
fn rows() -> Result<String, &'static str> {
    ipc::call(TERMINAL, READ_ROWS.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "founder-walk: the Terminal did not answer its read")
}

/// The Terminal's rows once they hold `text`.
fn rows_show(text: &str) -> Result<String, &'static str> {
    let mut seen = String::new();
    for _ in 0..REPLY_MS / POLL_MS {
        seen = rows()?;
        if seen.contains(text) {
            return Ok(seen);
        }
        threads::sleep_ms(POLL_MS);
    }
    log!(
        "{CASE}: waited for {text:?}; the Terminal's rows are {}",
        seen.replace('\n', "; ")
    );
    Err("founder-walk: the Terminal did not show the answer")
}

/// Step 1: the Terminal from the panel; the typed line shows in its input
/// row before Enter, and Enter runs it.
fn terminal_runs_the_line() -> Result<(), &'static str> {
    panel_line("open terminal");
    desk_tests::wait("founder-walk: the Terminal to start", || {
        !process::named(TERMINAL).is_empty()
    })?;
    threads::sleep_ms(OPEN_MS);
    log!("{CASE}: the Terminal opened from the panel");
    let (before, across) = still_screen()?;
    type_text(CPUS_LINE);
    threads::sleep_ms(SETTLE_MS);
    let (typed, _) = signature()?;
    let changed = most_changed_in_a_row(&before, &typed, across);
    log!("{CASE}: typing the line changed {changed} squares of one row of the screen");
    if changed < MIN_TYPED_TILES {
        return Err("founder-walk: the Terminal's input row did not show the typed line");
    }
    type_keys(&ENTER);
    rows_show("CPUs: ")?;
    log!("{CASE}: the Terminal ran the typed line and showed the CPUs");
    Ok(())
}

/// Step 2: abilities from the rows, and a clarify cut to what rows back.
fn abilities_and_clarify() -> Result<(), &'static str> {
    window_line(ABILITIES_LINE);
    // The answer names the topic by its stem (`shellcore::abilities`).
    rows_show("With the cpu, I can:")?;
    log!("{CASE}: what Kai can do with the cpus came from the rows");
    window_line(CLARIFY_LINE);
    let rows = rows_show(&format!("({BACKED})"))?;
    if rows.contains(UNBACKED) {
        return Err("founder-walk: the clarify offered an option no row backs");
    }
    log!("{CASE}: the model's clarify offered only what a row backs");
    Ok(())
}

/// Waits for the ask on the card, then past its settle time `y` arms Run and
/// a second `y` answers it.
fn yes_on_the_card() -> Result<(), &'static str> {
    desk_tests::wait("founder-walk: the ask on the card", ask_box::shown)?;
    threads::sleep_ms(termcore::menu::SETTLE_MS + 300);
    type_keys(&YES);
    threads::sleep_ms(KEY_MS);
    type_keys(&YES);
    desk_tests::wait("founder-walk: the card to take its answer", || {
        !ask_box::shown()
    })
}

/// Waits for `path` in `fs` to hold [`FILE_TEXT`], read by the shell.
fn file_made(path: &str) -> Result<(), &'static str> {
    let read = format!("fs_read(\"{path}\")");
    let mut text = String::new();
    for _ in 0..REPLY_MS / POLL_MS {
        let reply = ipc::call("shell", read.as_bytes(), CALL_MS)
            .map_err(|_| "founder-walk: the shell did not answer the read")?;
        text = String::from_utf8_lossy(&reply).into_owned();
        if text.contains(FILE_TEXT) {
            return Ok(());
        }
        threads::sleep_ms(POLL_MS);
    }
    log!("{CASE}: {read} answered {text:?}");
    Err("founder-walk: the file the card allowed is not in fs")
}

/// Step 3: a line that asks first, from the Terminal and from the panel; the
/// Terminal's is also the line after the clarify, a request of its own.
fn lines_ask_first() -> Result<(), &'static str> {
    window_line(TERMINAL_ASK);
    yes_on_the_card()?;
    file_made(TERMINAL_FILE)?;
    log!("{CASE}: the Terminal's line asked first, and Run on the card made its file");
    panel_line(PANEL_ASK);
    yes_on_the_card()?;
    file_made(PANEL_FILE)?;
    log!("{CASE}: the panel's line asked first, and Run on the card made its file");
    Ok(())
}

/// Notes' count of its notes, or nothing while it cannot answer.
fn notes_count() -> String {
    ipc::call(NOTES, COUNT_NOTES.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .unwrap_or_default()
}

/// Step 4: a note from the panel reaches Notes, which nothing had started.
fn note_reaches_notes() -> Result<(), &'static str> {
    if !process::named(NOTES).is_empty() {
        return Err("founder-walk: Notes ran before anything asked for it");
    }
    panel_line(NOTE_LINE);
    desk_tests::wait("founder-walk: Notes to start", || {
        !process::named(NOTES).is_empty()
    })?;
    let mut count = String::new();
    for _ in 0..REPLY_MS / POLL_MS {
        threads::sleep_ms(POLL_MS);
        count = notes_count();
        if count.contains("notes: 1") {
            log!("{CASE}: the note reached Notes on a fresh boot");
            return Ok(());
        }
    }
    log!("{CASE}: Notes' count answered {count:?}");
    Err("founder-walk: the note did not reach Notes")
}

/// A click at `at`, the pointer brought from the screen's top left corner.
fn click(at: (i32, i32)) {
    report(TO_CORNER, TO_CORNER, 0);
    report(at.0, at.1, 0);
    report(0, 0, 1);
    report(0, 0, 0);
}

/// Step 5: Notes' window opened, closed from its control, and asked for
/// again; the host checks that it came back.
fn notes_open_close_open() -> Result<(), &'static str> {
    panel_line("open notes");
    threads::sleep_ms(OPEN_MS);
    click(NOTES_CLOSE);
    threads::sleep_ms(SETTLE_MS);
    log!("{CASE}: Notes' window was closed from its control");
    panel_line("open notes");
    threads::sleep_ms(OPEN_MS);
    if process::named(NOTES).is_empty() {
        return Err("founder-walk: Notes ended instead of coming back");
    }
    log!("{CASE}: Notes was asked for again");
    Ok(())
}

pub fn check() -> Result<(), &'static str> {
    desk_tests::wait("founder-walk: the desktop to draw", || {
        desk_tests::desktop_shows() && claim::flushes() > 0
    })?;
    desk_tests::wait("founder-walk: the shell and the card", || {
        ipc::server_of("shell").is_some() && ask_box::alive()
    })?;
    terminal_runs_the_line()?;
    abilities_and_clarify()?;
    lines_ask_first()?;
    note_reaches_notes()?;
    notes_open_close_open()?;
    log!("{CASE}: the walk is done");
    Ok(())
}
