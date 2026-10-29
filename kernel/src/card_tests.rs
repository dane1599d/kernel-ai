//! `test=desk-card` (P3 task 7, ADR 0005 §7): asks answered on `desktop`'s
//! card. `deskdemo`'s windows are open and keep committing; each ask runs on a
//! thread of its own, as the terminal's would.
//! - With `desktop` showing, the ask goes to the card and never to the console:
//!   the screen stays `desktop`'s, and `y` answers it Allow.
//! - A letter and Enter do not answer a card; `n` then answers it Deny.
//! - With a card up, `desktop` is killed: the kernel takes the ask back and
//!   asks it on the console, where `n` answers it.

use crate::display::claim;
use crate::keyboard::Decoder;
use crate::{ask_box, log, process, terminal, threads};
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use termcore::menu::{Answer, Menu, Origin};

const WAIT_MS: u64 = 60_000;
/// Time for a key to reach `desktop` and its answer to come back.
const KEY_MS: u64 = 1_000;

/// The ask's answer: 0 while it waits.
static ANSWER: AtomicU8 = AtomicU8::new(0);
/// The id the next ask uses.
static NEXT: AtomicU64 = AtomicU64::new(1);

fn code(answer: Answer) -> u8 {
    match answer {
        Answer::Allow => 1,
        Answer::Deny => 2,
        _ => 3,
    }
}

/// The question the shell asks for it: a menu's question shows its call.
const QUESTION: &str = "Run process_stop(\"build-3\")? It stops a process.";

/// Asks for `process_stop("build-3")`, as the terminal does.
fn asker(_: usize) {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut decoder = Decoder::new();
    let answer = match Menu::new(id, QUESTION, None, None, Origin::Asked)
        .and_then(|menu| menu.with_call("process_stop(\"build-3\")"))
    {
        Ok(mut menu) => code(terminal::answer(&mut decoder, &mut menu)),
        Err(_) => 9,
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
    log!("desk-card: timed out waiting for {what}");
    Err(what)
}

/// Starts an ask and waits until the card shows it.
fn ask_on_card() -> Result<(), &'static str> {
    ANSWER.store(0, Ordering::Release);
    threads::spawn("asker", asker, 0)?;
    wait("the ask on the card", ask_box::shown)?;
    threads::sleep_ms(KEY_MS);
    Ok(())
}

fn answered() -> Result<u8, &'static str> {
    wait("the answer", || ANSWER.load(Ordering::Acquire) != 0)?;
    Ok(ANSWER.load(Ordering::Acquire))
}

pub fn check() -> Result<(), &'static str> {
    wait("desktop, its trusted layer and deskdemo", || {
        claim::showing() && ask_box::alive() && !process::named("deskdemo").is_empty()
    })?;
    // deskdemo's windows open with its first call.
    threads::sleep_ms(2_000);
    // Y.
    ask_on_card()?;
    crate::claim_tests::type_keys(&[0x15, 0x95]);
    if answered()? != code(Answer::Allow) || !claim::showing() {
        return Err("desk-card: y on the card did not allow, or the console took the screen");
    }
    log!("desk-card: an ask went to desktop's card, and y on it allowed");
    // A, Enter, then N.
    ask_on_card()?;
    crate::claim_tests::type_keys(&[0x1E, 0x9E, 0x1C, 0x9C]);
    threads::sleep_ms(KEY_MS);
    if ANSWER.load(Ordering::Acquire) != 0 {
        return Err("desk-card: a letter or Enter answered the card");
    }
    crate::claim_tests::type_keys(&[0x31, 0xB1]);
    if answered()? != code(Answer::Deny) || !claim::showing() {
        return Err("desk-card: n on the card did not deny");
    }
    log!("desk-card: a letter and Enter did not answer the card, and n denied");
    // desktop killed with a card up.
    ask_on_card()?;
    let &(pid, _) = process::named("desktop")
        .first()
        .ok_or("desk-card: desktop is gone")?;
    process::kill(pid, kabi::ExitReason::Killed).map_err(|_| "desk-card: cannot kill desktop")?;
    wait("the ask back on the console", || {
        !claim::showing() && !ask_box::shown()
    })?;
    threads::sleep_ms(KEY_MS);
    crate::claim_tests::type_keys(&[0x31, 0xB1]);
    if answered()? != code(Answer::Deny) {
        return Err("desk-card: the ask did not come back to the console");
    }
    log!("desk-card: with desktop killed, the ask came back and the console answered it");
    Ok(())
}
