//! `test=desk-start` (Kai Desk PR 8): a Super tap opens Start with its box
//! focused; "not" selects Notes, the first app whose name starts with it;
//! Enter sends the shell `open notes`, which starts Notes' window service
//! through `ui_open`, with no model. `desktop` logs the focus, the catalog
//! and the line it sent, which the host checks, and never queues a request
//! for the model.

use crate::agent_desk_tests::{OPEN_MS, wait};
use crate::{claim_tests, log, process, terminal, threads};

/// Left Super down and up (set 1, E0-prefixed).
const SUPER: [u8; 4] = [0xE0, 0x5B, 0xE0, 0xDB];
/// "not", then Enter: make and break codes.
const NOT_ENTER: [u8; 8] = [0x31, 0xB1, 0x18, 0x98, 0x14, 0x94, 0x1C, 0x9C];
/// Time for `desktop` to take a step.
const STEP_MS: u64 = 600;

pub fn check() -> Result<(), &'static str> {
    const CASE: &str = "desk-start";
    threads::spawn("terminal", terminal::run, 0)?;
    wait(CASE, "desktop and term", || {
        crate::display::claim::showing() && !process::named("term").is_empty()
    })?;
    // term opens its window with its first call.
    threads::sleep_ms(3_000);
    if !process::named("notes-window").is_empty() {
        return Err("desk-start: notes-window ran before Start asked for it");
    }
    claim_tests::type_keys(&SUPER);
    threads::sleep_ms(STEP_MS);
    log!("{CASE}: a Super tap was typed");
    claim_tests::type_keys(&NOT_ENTER);
    wait(CASE, "Start to open Notes", || {
        !process::named("notes-window").is_empty()
    })?;
    threads::sleep_ms(OPEN_MS);
    log!("desk-start: Super, \"not\" and Enter opened Notes from Start");
    Ok(())
}
