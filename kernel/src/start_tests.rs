//! `test=desk-start` and `test=desk-start-esc` (Kai Desk PR 8): with the
//! Terminal's window holding the keys on a lease, as at every boot, a
//! Super tap (or Ctrl+Esc) still reaches `desktop` and ends the lease
//! (`kabi::input::Set1::desk_chord`); Start opens with its box focused;
//! "not" selects Notes, the first app whose name starts with it; Enter
//! sends the shell `open notes`, which starts Notes' window service through
//! `ui_open`, with no model. `desktop` logs the focus, the catalog and the
//! line it sent, which the host checks, and never queues a request for the
//! model.

use crate::agent_desk_tests::{OPEN_MS, wait};
use crate::{claim_tests, input, log, process, terminal, threads};

/// Left Super down and up (set 1, E0-prefixed).
const SUPER: &[u8] = &[0xE0, 0x5B, 0xE0, 0xDB];
/// Left Ctrl down, Esc down and up, Ctrl up.
const CTRL_ESC: &[u8] = &[0x1D, 0x01, 0x81, 0x9D];
/// "not", then Enter: make and break codes.
const NOT_ENTER: [u8; 8] = [0x31, 0xB1, 0x18, 0x98, 0x14, 0x94, 0x1C, 0x9C];

pub fn check() -> Result<(), &'static str> {
    open_from_start("desk-start", "a Super tap", SUPER)
}

pub fn esc_check() -> Result<(), &'static str> {
    open_from_start("desk-start-esc", "Ctrl+Esc", CTRL_ESC)
}

fn open_from_start(case: &str, what: &str, chord: &[u8]) -> Result<(), &'static str> {
    threads::spawn("terminal", terminal::run, 0)?;
    wait(case, "desktop and term", || {
        crate::display::claim::showing() && !process::named("term").is_empty()
    })?;
    // term opens its window with its first call, which takes the keys.
    wait(case, "the terminal's key lease", input::leased)?;
    if !process::named("notes-window").is_empty() {
        return Err("desk-start: notes-window ran before Start asked for it");
    }
    log!("{case}: the terminal holds the keys");
    claim_tests::type_keys(chord);
    // The chord ended the lease: the next keys go to Start, not the console.
    wait(case, "the chord to end the lease", || !input::leased())?;
    log!("{case}: {what} reached desktop through the lease");
    threads::sleep_ms(600);
    claim_tests::type_keys(&NOT_ENTER);
    wait(case, "Start to open Notes", || {
        !process::named("notes-window").is_empty()
    })?;
    threads::sleep_ms(OPEN_MS);
    log!("{case}: \"not\" and Enter opened Notes from Start");
    Ok(())
}
