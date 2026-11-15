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
//!    note), and it asks first: `y` twice on the trusted card, the Terminal's
//!    row says what it came to, and its file is in `fs`. The same from the
//!    panel, whose bar the host reads (`log_bar=1`).
//! 4. "add a note saying hello dane" in the panel reaches Notes, which nothing
//!    had started.
//! 5. `open notes` shows its window, its close control closes it, and `open
//!    notes` brings it back.

use crate::claim_tests::{type_keys, type_text};
use crate::desk_tests::{self, report};
use crate::display::claim;
use crate::{ask_box, display, ipc, log, process, terminal, threads};
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
/// `y` presses on the card, more than its quiet time apart: a press in that
/// time only pushes it back, so the card counts from its draw, not its take.
const YES_GAP_MS: u64 = termcore::menu::SETTLE_MS + 300;
/// Most `y` presses the card may need: one arms Run and one answers it, and
/// the rest cover a slow first draw.
const MAX_YES: usize = 12;
/// How often the card's answer and the typed line's pixels are looked for.
const LOOK_MS: u64 = 50;
/// How long the typed line may take to show in the Terminal's input row.
const TYPED_MS: u64 = 10_000;
/// Ctrl down, Space down and up, Ctrl up: the panel takes the keys.
const CTRL_SPACE: [u8; 4] = [0x1D, 0x39, 0xB9, 0x9D];
/// Backspace down and up, a few times: a `y` that reached the panel after
/// its card was answered is taken back before the next line.
const CLEAR: [u8; 6] = [0x0E, 0x8E, 0x0E, 0x8E, 0x0E, 0x8E];
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
/// The model's clarify question, and its options: one a row backs, and one
/// none does.
const CLARIFY_QUESTION: &str = "What should I do with the CPUs?";
const BACKED: &str = "show the cpus";
const UNBACKED: &str = "increase CPU frequency";
/// Lines that name the change but not its file, so each asks first.
const TERMINAL_ASK: &str = "save a note for the terminal";
const TERMINAL_FILE: &str = "walk-term.txt";
const PANEL_ASK: &str = "save a note for the walk";
const PANEL_FILE: &str = "walk-panel.txt";
/// What the model writes in both files.
const FILE_TEXT: &str = "hello from the walk";
/// Reads of a file the card allowed, this far apart.
const FILE_READS: usize = 3;
const FILE_READ_GAP_MS: u64 = 5_000;
const NOTE_LINE: &str = "add a note saying hello dane";

/// The side of a square of the screen [`signature`] hashes as one.
const TILE: usize = 16;
/// The fewest squares of one row of them that typing the line must change:
/// its text spans a dozen or more, a blinking caret one or two.
const MIN_TYPED_TILES: usize = 6;
/// Tries at a still screen before the typing, [`SETTLE_MS`] apart.
const STILL_TRIES: usize = 10;

/// Notes' window is the desk's second: the Terminal's took the cascade's
/// first spot (deskgeom counts spots from 0).
const NOTES_SPOT: i32 = 1;

/// The walk's plan: the default boot's `system`, with `log_bar=1` on
/// `desktop`'s line, so it logs each new line of the panel's bar.
pub fn plan(system: &str) -> String {
    system
        .lines()
        .map(|line| match line.starts_with("desktop ") {
            true => format!("{line} log_bar=1"),
            false => String::from(line),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

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
    type_keys(&CLEAR);
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

/// Waits for the typed line to change one row of the screen's squares from
/// `before` by [`MIN_TYPED_TILES`] or more.
fn typed_line_shows(before: &[u64], across: usize) -> Result<(), &'static str> {
    let mut changed = 0;
    for _ in 0..TYPED_MS / LOOK_MS {
        let (typed, _) = signature()?;
        changed = most_changed_in_a_row(before, &typed, across);
        if changed >= MIN_TYPED_TILES {
            log!("{CASE}: typing the line changed {changed} squares of one row of the screen");
            return Ok(());
        }
        threads::sleep_ms(LOOK_MS);
    }
    log!("{CASE}: typing the line changed {changed} squares of one row at most");
    Err("founder-walk: the Terminal's input row did not show the typed line")
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
    typed_line_shows(&before, across)?;
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
    let rows = rows_show(CLARIFY_QUESTION)?;
    if rows.contains(UNBACKED) {
        return Err("founder-walk: the clarify offered an option no row backs");
    }
    if !rows.contains(&format!("{CLARIFY_QUESTION} ({BACKED})")) {
        log!("{CASE}: the clarify showed as {}", rows.replace('\n', "; "));
        return Err("founder-walk: the clarify dropped the option a row backs");
    }
    log!("{CASE}: the model's clarify offered only what a row backs");
    Ok(())
}

/// What the shell answers `read <path>`, which the mock model answers with
/// `fs_read` (as `act-fs` reads): a typed Rhai line binds no file rows.
fn file_text(path: &str) -> Result<String, &'static str> {
    let read = format!("read {path}");
    ipc::call("shell", read.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "founder-walk: the shell did not answer the read")
}

/// Whether the card took its answer within `ms`.
fn answered_within(ms: u64) -> bool {
    for _ in 0..ms / LOOK_MS {
        if !ask_box::shown() {
            return true;
        }
        threads::sleep_ms(LOOK_MS);
    }
    !ask_box::shown()
}

/// Waits for the ask on the card, which must name `path`, then presses `y`
/// [`YES_GAP_MS`] apart until the card takes its answer (one arms Run and the
/// next answers it), and waits for the shell's answer to it: the run is over
/// once the terminal shows one more of the shell's replies.
///
/// Nothing reads `path` while the card is up: any other console line would
/// discard the proposal the card answers. That nothing made the file before
/// Run shows in the run itself: `fs_create` never overwrites, and the host
/// expects the panel's bar to say it created the file.
fn yes_on_the_card(path: &str) -> Result<(), &'static str> {
    desk_tests::wait("founder-walk: the ask on the card", ask_box::shown)?;
    let replies = terminal::replies_shown();
    let question = ask_box::question().unwrap_or_default();
    if !question.contains(path) {
        log!("{CASE}: the card asks {question:?}");
        return Err("founder-walk: the card does not name the file it makes");
    }
    let mut answered = false;
    for _ in 0..MAX_YES {
        type_keys(&YES);
        if answered_within(YES_GAP_MS) {
            answered = true;
            break;
        }
    }
    if !answered {
        return Err("founder-walk: the card never took its answer");
    }
    desk_tests::wait("founder-walk: the shell's answer to the card", || {
        terminal::replies_shown() > replies
    })
}

/// Checks that `path` in `fs` holds [`FILE_TEXT`]. The run is over once the
/// card's answer is (`yes_on_the_card`), so a few reads far apart do: each is
/// a line to the shell, and a model's round.
fn file_made(path: &str) -> Result<(), &'static str> {
    let mut text = String::new();
    for _ in 0..FILE_READS {
        text = file_text(path)?;
        if text.contains(FILE_TEXT) {
            return Ok(());
        }
        threads::sleep_ms(FILE_READ_GAP_MS);
    }
    log!("{CASE}: the read of {path} answered {text:?}");
    Err("founder-walk: the file the card allowed is not in fs")
}

/// Step 3: a line that asks first, from the Terminal and from the panel; the
/// Terminal's is also the line after the clarify, a request of its own.
fn lines_ask_first() -> Result<(), &'static str> {
    window_line(TERMINAL_ASK);
    yes_on_the_card(TERMINAL_FILE)?;
    // What the card came to shows where the line was typed: code's done
    // line, without its undo (`shellcore::deskask`), in the Terminal's row.
    rows_show(&format!("Done: file {TERMINAL_FILE} created (was absent)."))?;
    file_made(TERMINAL_FILE)?;
    log!(
        "{CASE}: the Terminal's line asked first, Run on the card made its file, and the Terminal showed what it came to"
    );
    panel_line(PANEL_ASK);
    yes_on_the_card(PANEL_FILE)?;
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

/// The desk's points at the screen's size, which the case's `display=` sets
/// (deskgeom), so a layout change moves the walk's clicks with it.
fn desk() -> Result<deskgeom::Points, &'static str> {
    let surface = display::surface().ok_or("founder-walk: no display")?;
    let px = |n: usize| i32::try_from(n).map_err(|_| "founder-walk: the screen is too big");
    Ok(deskgeom::points(px(surface.width)?, px(surface.height)?))
}

/// A click at `at`: the pointer pinned to deskgeom's corner, wherever it was,
/// and moved from there.
fn click(desk: &deskgeom::Points, at: (i32, i32)) {
    let (dx, dy) = desk.pin_to_kai();
    report(dx, dy, 0);
    let from = desk.pinned();
    report(at.0 - from.0, at.1 - from.1, 0);
    report(0, 0, 1);
    report(0, 0, 0);
}

/// Whether the second `open notes` brought Notes' window back: comphost
/// answers a show "ok shown (its window is open)" when it is, and a bare "ok
/// shown" when the show itself had to put it back, so the open had failed.
fn notes_came_back() -> Result<(), &'static str> {
    let answer = ipc::call(NOTES, kabi::live::SHOW.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "founder-walk: Notes did not answer after open notes")?;
    if answer.contains("its window is open") {
        return Ok(());
    }
    log!("{CASE}: after open notes, a show answered {answer:?}");
    Err("founder-walk: open notes did not bring Notes' window back")
}

/// Step 5: Notes' window opened, closed from its control, and asked for
/// again; it must be open again, as the host also checks.
fn notes_open_close_open() -> Result<(), &'static str> {
    let desk = desk()?;
    panel_line("open notes");
    threads::sleep_ms(OPEN_MS);
    click(&desk, desk.close(NOTES_SPOT, crate::ui_tests::APP_FRAME_W));
    threads::sleep_ms(SETTLE_MS);
    log!("{CASE}: Notes' window was closed from its control");
    panel_line("open notes");
    threads::sleep_ms(OPEN_MS);
    if process::named(NOTES).is_empty() {
        return Err("founder-walk: Notes ended instead of coming back");
    }
    notes_came_back()?;
    log!("{CASE}: Notes' window came back when asked again");
    Ok(())
}

pub fn check() -> Result<(), &'static str> {
    // The kernel's terminal, as the real boot runs it after `boot-ok`: it
    // takes a desktop line's waiting card from the shell and posts it on
    // `desktop`'s card. A test boot starts none of its own.
    threads::spawn("terminal", terminal::run, 0)?;
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
