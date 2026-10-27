//! `test=desk-ask` (P3 task 8, ADR 0005 §7): an ask is answered only on the
//! console. `desktop` holds the screen; the harness types `y` and Enter as if
//! into a window, first with the keys leased to the console (the terminal
//! window focused), then with no lease (focus elsewhere). Each time an Allow /
//! Deny menu then waits: the console must take the screen, the lease must end,
//! the keys typed before must not answer, and the `n` typed once the console
//! shows must, after which `desktop` gets the screen back. Then Ctrl+Alt+F12
//! gives the console the screen before an ask, and again while it waits: the
//! screen must stay on the console until the answer. Last, `desktop` is killed
//! with the lease on: the new `desktop` claims the screen with no lease, and
//! keys reach its queue.

use crate::display::claim;
use crate::keyboard::Decoder;
use crate::{console, input, log, process, terminal, threads};
use core::sync::atomic::{AtomicU8, Ordering};
use termcore::menu::{Answer, Menu, Origin};

const WAIT_MS: u64 = 60_000;
/// How long the harness's keys take to arrive after "ready for keys", with
/// room: it connects, waits 200 ms, then sends a key each 150 ms.
const KEYS_MS: u64 = 2_000;

/// The menu's answer: 0 while it waits.
static ANSWER: AtomicU8 = AtomicU8::new(0);
const REFUSED: u8 = 9;

fn code(answer: Answer) -> u8 {
    match answer {
        Answer::Allow => 1,
        Answer::Deny => 2,
        Answer::AlwaysKind => 3,
        Answer::AlwaysTarget => 4,
    }
}

/// Asks, as the terminal does, on a thread of its own.
fn asker(_: usize) {
    let mut decoder = Decoder::new();
    let answer = match Menu::new(1, "Reboot? It has no undo.", None, None, Origin::Asked) {
        Ok(mut menu) => code(terminal::answer_on_console(&mut decoder, &mut menu)),
        Err(_) => REFUSED,
    };
    ANSWER.store(answer, Ordering::Release);
}

fn wait(what: &'static str, mut done: impl FnMut() -> bool) -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        if done() {
            return Ok(());
        }
        threads::sleep_ms(50);
    }
    log!("desk-ask: timed out waiting for {what}");
    Err(what)
}

/// The `desktop` that shows now, if one does.
fn showing() -> Option<u64> {
    let &(pid, _) = process::named("desktop").first()?;
    let slot = process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid))?;
    claim::shows(slot).then_some(pid)
}

/// Waits `ms`, renewing the lease as `desktop` does while the window has focus.
/// The kernel's own `input::lease`, under INPUT_TO_CONSOLE's checks.
fn hold(ms: u64, leased: bool) {
    for _ in 0..ms / 100 {
        if leased {
            input::lease(true);
        }
        threads::sleep_ms(100);
    }
}

/// One ask after `y` and Enter were typed with the keys `leased` or not.
fn round(leased: bool) -> Result<(), &'static str> {
    let how = if leased {
        "leased to the console"
    } else {
        "with desktop"
    };
    wait("desktop to show", || showing().is_some())?;
    let before = input::keys();
    if leased {
        input::lease(true);
    }
    log!("desk-ask: ready for keys");
    hold(KEYS_MS, leased);
    let queued = input::keys() > before;
    if queued == leased {
        log!("desk-ask: keys typed {how} went the wrong way");
        return Err("desk-ask: the lease did not decide where keys went");
    }
    ANSWER.store(0, Ordering::Release);
    threads::spawn("desk-ask", asker, 0)?;
    wait("the console to take the screen for the ask", || {
        terminal::awaiting_answer() && showing().is_none() && !console::suspended()
    })?;
    if input::leased() {
        return Err("desk-ask: the lease outlived the ask's takeover");
    }
    // The y and Enter typed before cannot answer it.
    threads::sleep_ms(1_000);
    if ANSWER.load(Ordering::Acquire) != 0 {
        return Err("desk-ask: keys typed before the menu showed answered it");
    }
    log!("desk-ask: ready for keys");
    wait("the answer typed on the console", || {
        ANSWER.load(Ordering::Acquire) != 0
    })?;
    if ANSWER.load(Ordering::Acquire) != code(Answer::Deny) {
        return Err("desk-ask: the menu took an answer other than the n typed");
    }
    wait("desktop to get the screen back", || showing().is_some())?;
    log!("desk-ask: an ask after keys {how} was answered only on the console");
    Ok(())
}

/// Ctrl+Alt+F12, an ask, Ctrl+Alt+F12 again: the console had the screen
/// before the ask, and keeps it until the answer.
fn console_first() -> Result<(), &'static str> {
    wait("desktop to show", || showing().is_some())?;
    claim::switch();
    wait("the console to take the screen", || {
        showing().is_none() && !console::suspended()
    })?;
    ANSWER.store(0, Ordering::Release);
    threads::spawn("desk-ask", asker, 0)?;
    wait("the ask", terminal::awaiting_answer)?;
    // The second press would give the screen back; while the ask waits it
    // must not.
    claim::switch();
    threads::sleep_ms(1_000);
    if showing().is_some() {
        return Err("desk-ask: Ctrl+Alt+F12 gave the screen back while an ask waited");
    }
    log!("desk-ask: ready for keys");
    wait("the answer typed on the console", || {
        ANSWER.load(Ordering::Acquire) != 0
    })?;
    if ANSWER.load(Ordering::Acquire) != code(Answer::Deny) {
        return Err("desk-ask: the menu took an answer other than the n typed");
    }
    threads::sleep_ms(500);
    if showing().is_some() {
        return Err("desk-ask: the answer gave away a screen the console had before");
    }
    claim::switch();
    wait("desktop to get the screen back", || showing().is_some())?;
    log!("desk-ask: with the console showing already, the ask kept the screen until its answer");
    Ok(())
}

pub fn check() -> Result<(), &'static str> {
    round(true)?;
    round(false)?;
    console_first()?;
    // desktop dies holding the lease: the new one starts with none.
    let Some(first) = showing() else {
        return Err("desk-ask: desktop does not show");
    };
    input::lease(true);
    if process::kill(first, kabi::ExitReason::Killed).is_err() {
        return Err("desk-ask: cannot kill desktop");
    }
    wait("a new desktop to claim the screen", || {
        showing().is_some_and(|pid| pid != first)
    })?;
    if input::leased() {
        return Err("desk-ask: the lease outlived the desktop that held it");
    }
    let before = input::keys();
    log!("desk-ask: ready for keys");
    hold(KEYS_MS, false);
    if input::keys() == before {
        return Err("desk-ask: keys did not reach the new desktop");
    }
    log!("desk-ask: the restarted desktop got the keys, with no lease left over");
    Ok(())
}
