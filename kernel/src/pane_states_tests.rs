//! `test=pane-states`: the Kai pane through each of its states, from lines a
//! person types into it, on the v0.1 system as the default boot plans it with
//! the mock model and `desktop` logging its states (`log_bar=1`,
//! `founder_walk_tests::plan`). The host reads the desktop's lines for each:
//! 1. idle, as the pane starts;
//! 2. thinking, from a line's Enter;
//! 3. done, for a run-tier line that changes the policy with no card (this
//!    side checks the card never came up while the policy moved);
//! 4. refused, for a line code refuses before any model: `c` reads as a
//!    script naming a variable there is none of (the kernel types no Shift,
//!    so it cannot type `:c`);
//! 5. asking, for a line that asks first: its question goes on the card, and
//!    Run there makes its file, which the bar then says.

use crate::founder_walk_tests::{file_made, panel_line, yes_on_the_card};
use crate::{ask_box, desk_tests, ipc, log, sched_control, terminal, threads};

const CASE: &str = "pane-states";
/// A line the mock model answers with a run-tier script, no card
/// (`xtask/src/mock_llm.rs`).
const RUN_LINE: &str = "turn on round robin scheduling";
/// A line code refuses before any model: a script that names no variable.
const REFUSED_LINE: &str = "c";
/// A line that names a change but not its file, so it asks first, and the
/// file Run on the card makes (the mock model's, as the walk's).
const ASK_LINE: &str = "save a note for the walk";
const ASK_FILE: &str = "walk-panel.txt";
/// How long a line may take to show its answer, a model's rounds included.
const REPLY_MS: u64 = 60_000;
/// How often the policy and the card are looked at.
const LOOK_MS: u64 = 50;
/// How long the card is watched after the run-tier line's change: a card
/// that came late would still be up by then.
const AFTER_MS: u64 = 3_000;
/// Time for a refused line to reach the shell and come back.
const SETTLE_MS: u64 = 5_000;

/// Whether the scheduler runs round-robin.
fn round_robin() -> bool {
    let (spec, _) = sched_control::installed();
    sched_control::ids(spec).0 == kabi::policy::ROUND_ROBIN
}

/// Step 3: a run-tier line changes the policy, and the card never comes up,
/// before the change or for [`AFTER_MS`] after it.
fn run_tier_shows_no_card() -> Result<(), &'static str> {
    if round_robin() {
        return Err("pane-states: the policy was round-robin before the line");
    }
    panel_line(RUN_LINE);
    let mut changed = false;
    for _ in 0..REPLY_MS / LOOK_MS {
        if ask_box::shown() {
            return Err("pane-states: the run-tier line put a question on the card");
        }
        if round_robin() {
            changed = true;
            break;
        }
        threads::sleep_ms(LOOK_MS);
    }
    if !changed {
        return Err("pane-states: the run-tier line never changed the policy");
    }
    for _ in 0..AFTER_MS / LOOK_MS {
        if ask_box::shown() {
            return Err("pane-states: a card came up after the run-tier line ran");
        }
        threads::sleep_ms(LOOK_MS);
    }
    log!("{CASE}: the run-tier line changed the policy and showed no card");
    Ok(())
}

/// Step 4: a line the panel refuses; the host reads its refusal on the bar.
fn refused_line() -> Result<(), &'static str> {
    panel_line(REFUSED_LINE);
    threads::sleep_ms(SETTLE_MS);
    if ask_box::shown() {
        return Err("pane-states: the refused line put a question on the card");
    }
    log!("{CASE}: the panel was given a line it refuses");
    Ok(())
}

/// Step 5: a line that asks first; Run on the card makes its file.
fn asking_line() -> Result<(), &'static str> {
    panel_line(ASK_LINE);
    yes_on_the_card(ASK_FILE)?;
    file_made(ASK_FILE)?;
    log!("{CASE}: the line asked on the card, and Run there made its file");
    Ok(())
}

pub fn check() -> Result<(), &'static str> {
    // The kernel's terminal, as the real boot runs it: it takes a desktop
    // line's waiting card from the shell and posts it on `desktop`'s card.
    threads::spawn("terminal", terminal::run, 0)?;
    desk_tests::wait(
        "pane-states: the desktop to draw",
        desk_tests::desktop_shows,
    )?;
    desk_tests::wait("pane-states: the shell and the card", || {
        ipc::server_of("shell").is_some() && ask_box::alive()
    })?;
    run_tier_shows_no_card()?;
    refused_line()?;
    asking_line()?;
    log!("{CASE}: every state was shown");
    Ok(())
}
