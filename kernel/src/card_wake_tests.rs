//! `test=desk-card-wake` (Kai builds apps, follow-up 2): the kernel's terminal,
//! blocked at an empty prompt while `desktop` shows, wakes when the shell says
//! a card waits (CARD_READY), asks the shell for it with
//! `termcore::menu::TAKE_CARD`, and never while another ask is up. This
//! boot's shell has built no app, so it answers that none waits: the host
//! tests (`shellcore` `appcard`) cover the card itself.

use crate::desk_tests::wait;
use crate::display::claim;
use crate::{ask_box, ipc, log, terminal, threads};

/// Time for the terminal to reach its prompt and block on its keys.
const PROMPT_MS: u64 = 3_000;
/// How long a held card must stay unasked while another ask is up.
const HOLD_MS: u64 = 3_000;
/// The ask this case holds on the box, which no one answers.
const HELD_ID: u64 = 9_001;

pub fn check() -> Result<(), &'static str> {
    threads::spawn("terminal", terminal::run, 0)?;
    wait("desktop to read its ask box", || {
        claim::showing() && ask_box::alive()
    })?;
    wait("the shell", || ipc::server_of("shell").is_some())?;
    threads::sleep_ms(PROMPT_MS);
    // Another ask is up: the terminal wakes, and asks for no card.
    if !ask_box::post(HELD_ID, "held()", "Held by desk-card-wake?") {
        return Err("an ask was already posted");
    }
    terminal::card_ready();
    threads::sleep_ms(HOLD_MS);
    if terminal::cards_taken().1 != 0 {
        return Err("the terminal asked for a card while another ask was up");
    }
    ask_box::finish(true);
    // The shell says it again each pass; once the box is free, it is asked.
    terminal::card_ready();
    wait("the terminal to ask the shell for its card", || {
        terminal::cards_taken().1 == 1
    })?;
    if terminal::cards_taken().0 != 0 {
        return Err("the shell gave a card it did not have");
    }
    log!(
        "desk-card-wake: the prompt woke for the shell's card, waited while another ask was up, then asked the shell"
    );
    Ok(())
}
