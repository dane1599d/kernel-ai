//! `test=display-claim` (P3 task 2, ADR 0005 §2): the screen changes hands.
//! - `desktop` claims the screen: the console stops painting, and a key goes to
//!   the input queue, which `desktop` reports.
//! - Ctrl+Alt+F12 revokes the claim: the console repaints and takes keys again,
//!   and `desktop` hears it lost the screen. Until the second press nobody may
//!   claim it, and no process but `desktop` holds a `Display` handle: the plan's
//!   `fakedesk`, which asks for the screen under another name, never starts.
//! - A second Ctrl+Alt+F12 gives the screen back, and `desktop` claims it again.

use crate::display::claim;
use crate::{console, input, ipc, log, process, threads};
use x86_64::instructions::interrupts::without_interrupts;

/// The plan lines this case adds.
pub const PLAN: &str =
    "desktop caps=display+input+serve pages=8192 stack_pages=512\nfakedesk program=desktop caps=display+input";
const WAIT_MS: u64 = 60_000;
/// Ctrl, Alt, F12 down; F12, Alt, Ctrl up: set 1 make and break codes.
const CTRL_ALT_F12: [u8; 6] = [0x1D, 0x38, 0x58, 0xD8, 0xB8, 0x9D];
/// `a` down and up.
const KEY_A: [u8; 2] = [0x1E, 0x9E];

/// Keys as if the keyboard sent them. The case types nothing else, so the
/// ring's one producer is this thread for the moment.
fn type_keys(bytes: &[u8]) {
    let waiter = without_interrupts(|| {
        let waiter = crate::i8042::inject(bytes);
        crate::input::wake_pending();
        waiter
    });
    if let Some(tid) = waiter {
        without_interrupts(|| threads::wake(tid));
    }
}

fn wait(what: &'static str, mut done: impl FnMut() -> bool) -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 20 {
        if done() {
            return Ok(());
        }
        threads::sleep_ms(20);
    }
    log!("display-claim: timed out waiting for {what}");
    Err(what)
}

fn desktop_slot() -> Option<usize> {
    let pid = process::named("desktop").first()?.0;
    process::with_table(|table| table.iter().position(|p| p.live() && p.pid == pid))
}

pub fn check() -> Result<(), &'static str> {
    wait("desktop to claim the screen", || {
        desktop_slot().is_some_and(claim::shows) && ipc::server_of("shell").is_some()
    })?;
    if !console::suspended() || !input::taking() {
        return Err("display-claim: the console still holds the screen after the claim");
    }
    type_keys(&KEY_A);
    threads::sleep_ms(200);
    // Ctrl+Alt+F12: back to the console.
    type_keys(&CTRL_ALT_F12);
    wait("the console to take the screen back", || {
        !console::suspended() && !input::taking()
    })?;
    if desktop_slot().is_some_and(claim::shows) {
        return Err("display-claim: desktop still shows after Ctrl+Alt+F12");
    }
    // Nobody may claim it now, desktop included; fakedesk never started.
    let refused = desktop_slot().map(|slot| {
        let mut info = kabi::display::DisplayInfo::default();
        claim::claim(slot, &mut info)
    });
    if refused != Some(Err(kabi::Error::Busy)) {
        log!("display-claim: a claim after Ctrl+Alt+F12 gave {refused:?}");
        return Err("display-claim: the screen was claimable before the second press");
    }
    if !process::named("fakedesk").is_empty() {
        return Err("display-claim: a process not named desktop got the screen's handles");
    }
    // The second press: desktop claims again.
    type_keys(&CTRL_ALT_F12);
    wait("desktop to claim the screen again", || {
        desktop_slot().is_some_and(claim::shows) && console::suspended()
    })?;
    log!(
        "display-claim: desktop claimed the screen, Ctrl+Alt+F12 gave it to the console and back, and only desktop could hold it ({} input events dropped)",
        input::dropped()
    );
    Ok(())
}
