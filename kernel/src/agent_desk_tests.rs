//! `test=desk-agent` and `test=desk-takeover` (P3 task 10's exit cases, ADR
//! 0005 §6): the agent opens an app and acts inside it, and the user takes
//! over. Lines are typed in `term`'s window, where an ask shows on `desktop`'s
//! card; the mock model answers each by its line (`xtask/src/mock_llm.rs`).
//!
//! `desk-agent`: "open notes" runs `ui_open`, which starts Notes' window
//! service, `notes-window`, through the supervisor's `app-call` and waits for
//! its window; "search notes for milk" calls `ui_view`, the shell hands the view back for a second round,
//! and the model's `ui_act` on the search field asks; `y` on the card sends the
//! app an AccessKit action. `desk-takeover`: "switch to notes" runs
//! `ui_focus`, which gives Notes the agent's outline, and a key typed in Notes
//! stops the run and the panel says so. Each step shows in `desktop`'s,
//! comphost's and the shell's log lines, which the host checks.

use crate::desk_tests::report;
use crate::display::claim;
use crate::{ask_box, input, log, process, terminal, threads};

/// `desktop` as `desk-term` runs it, with the ask card (`ask`). Planned before
/// the shell, whose route to it is the shell's third.
pub const DESK: &str =
    "desktop caps=display+input+serve+ask pages=8192 stack_pages=512 burst_ms=10000";
/// The terminal's window, and Notes' window as an app that starts only when
/// `ui_open` asks for it.
pub const APPS: &str = "term uses=desktop caps=call+term pages=4096 stack_pages=512\n\
     notes-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1 start=demand";
const WAIT_MS: u64 = 60_000;
/// Where `desktop` starts the pointer: the middle of 1280x800.
const START: (i32, i32) = (640, 400);
/// The Terminal's title bar: its frame is the first, at 48,52, and Notes' the
/// second, at 80,84, below it.
const TERM_TITLE: (i32, i32) = (150, 60);
/// Time for comphost to start Notes and paint its first frame under TCG.
const OPEN_MS: u64 = 20_000;
/// Time for the harness's keys, and for the shell to take a line.
const KEYS_MS: u64 = 4_000;

fn wait(
    case: &str,
    what: &'static str,
    mut done: impl FnMut() -> bool,
) -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        if done() {
            return Ok(());
        }
        threads::sleep_ms(50);
    }
    log!("{case}: timed out waiting for {what}");
    Err(what)
}

/// Clicks at `to`, moving there from `at`.
fn click(at: &mut (i32, i32), to: (i32, i32)) {
    report(to.0 - at.0, to.1 - at.1, 0);
    *at = to;
    report(0, 0, 1);
    report(0, 0, 0);
}

/// Gives the Terminal window the keys, and asks the host for the next line.
fn terminal_keys(case: &str, at: &mut (i32, i32)) -> Result<(), &'static str> {
    click(at, TERM_TITLE);
    wait(
        case,
        "desktop to lend the keys to the terminal",
        input::leased,
    )?;
    log!("{case}: ready for keys");
    Ok(())
}

/// The desktop, the terminal's window, and Notes opened by `ui_open`.
fn open_notes(case: &str, at: &mut (i32, i32)) -> Result<(), &'static str> {
    threads::spawn("terminal", terminal::run, 0)?;
    wait(case, "desktop and term", || {
        claim::showing() && !process::named("term").is_empty()
    })?;
    // term opens its window with its first call.
    threads::sleep_ms(3_000);
    if !process::named("notes-window").is_empty() {
        return Err("notes-window ran before ui_open asked for it");
    }
    // "open notes"
    terminal_keys(case, at)?;
    wait(case, "ui_open to start Notes", || {
        !process::named("notes-window").is_empty()
    })?;
    threads::sleep_ms(OPEN_MS);
    log!("{case}: ui_open started Notes and its window opened");
    Ok(())
}

pub fn agent_check() -> Result<(), &'static str> {
    const CASE: &str = "desk-agent";
    let mut at = START;
    open_notes(CASE, &mut at)?;
    // "search notes for milk": a view, then an act that asks.
    terminal_keys(CASE, &mut at)?;
    wait(CASE, "the ask on desktop's card", ask_box::shown)?;
    threads::sleep_ms(KEYS_MS);
    // "y" on the card.
    log!("{CASE}: ready for keys");
    wait(CASE, "the card's answer", || !ask_box::shown())?;
    threads::sleep_ms(KEYS_MS);
    log!("desk-agent: the agent opened Notes, read its view and typed in it after a yes");
    Ok(())
}

pub fn takeover_check() -> Result<(), &'static str> {
    const CASE: &str = "desk-takeover";
    let mut at = START;
    open_notes(CASE, &mut at)?;
    // "switch to notes": Notes takes the focus and the outline.
    terminal_keys(CASE, &mut at)?;
    wait(CASE, "the focus to leave the terminal", || !input::leased())?;
    threads::sleep_ms(KEYS_MS);
    // A key typed in Notes, the window the agent drives.
    let keys = input::keys();
    log!("{CASE}: ready for keys");
    wait(CASE, "the key in Notes", || input::keys() > keys)?;
    threads::sleep_ms(KEYS_MS);
    if input::leased() {
        return Err("desk-takeover: the key went to the terminal, not Notes");
    }
    log!("desk-takeover: a key in the window the agent drove stopped it");
    Ok(())
}
