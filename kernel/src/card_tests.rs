//! `test=desk-card` (P3 task 7, ADR 0005 §7): asks answered on `desktop`'s
//! card. `deskdemo`'s windows are open and keep committing; each ask runs on a
//! thread of its own, as the terminal's would.
//! - With `desktop` showing, the ask goes to the card and never to the console:
//!   the screen stays `desktop`'s. Once it settled, one `y` only arms it, and
//!   a second answers it Allow.
//! - A letter and Enter do not answer a card; `n` then answers it Deny.
//! - A send's menu with no text beside it (#1142), asked by `answer` and by
//!   `choose`'s `pick`, its card withdrawn by Ctrl+Alt+F12: denied, and the
//!   console never takes a key for it.
//! - With a card up, `desktop` is killed: the kernel takes the ask back and
//!   asks it on the console, where `n`, typed once its menu takes keys,
//!   answers it.
//! - With no card up, the same send's menu is denied by both without a key
//!   read; another menu by `pick` is still asked on the console.

use crate::display::claim;
use crate::keyboard::Decoder;
use crate::{ask_box, log, process, terminal, threads};
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use termcore::menu::{Answer, Menu, Origin};

const WAIT_MS: u64 = 60_000;
/// Time for a key to reach `desktop` and its answer to come back.
const KEY_MS: u64 = 1_000;
/// Past the card's settle time, in which it takes no key (#864).
const SETTLE_MS: u64 = termcore::menu::SETTLE_MS + 300;

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

/// A send's call, and its question: a desk card's, its text not beside it.
const SEND_CALL: &str = "net_send(\"hooks.example\", \"/hook\", \"0123456789abcdef\")";
const SEND_QUESTION: &str =
    "Run net_send(\"hooks.example\", \"/hook\", \"0123456789abcdef\")? It sends text.";

/// The asks: `process_stop("build-3")` by `answer`, as the terminal does, a
/// send by `answer`, a send by `pick` with no text shown, and
/// `process_stop` by `pick`.
const STOP: usize = 0;
const SEND: usize = 1;
const SEND_PICKED: usize = 2;
const STOP_PICKED: usize = 3;

fn asker(kind: usize) {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut decoder = Decoder::new();
    let (question, call) = match kind {
        SEND | SEND_PICKED => (SEND_QUESTION, SEND_CALL),
        _ => (QUESTION, "process_stop(\"build-3\")"),
    };
    let answer = match Menu::new(id, question, None, None, Origin::Asked)
        .and_then(|menu| menu.with_call(call))
    {
        Ok(mut menu) if kind == SEND_PICKED || kind == STOP_PICKED => {
            code(terminal::pick(&mut decoder, &mut menu, None))
        }
        Ok(mut menu) => code(terminal::answer(&mut decoder, &mut menu)),
        Err(_) => 9,
    };
    ANSWER.store(answer, Ordering::Release);
}

/// A send's ask is answered Deny with no key typed, and the console's menu
/// never took keys for it.
fn denied_unread(what: &'static str) -> Result<(), &'static str> {
    let mut read = false;
    wait("the send's answer", || {
        read |= terminal::menu_takes_keys();
        ANSWER.load(Ordering::Acquire) != 0
    })?;
    if read || ANSWER.load(Ordering::Acquire) != code(Answer::Deny) {
        log!("desk-card: {what}: not denied, or the console read keys for it");
        return Err("desk-card: a send without its text was not denied unread");
    }
    Ok(())
}

/// A send's ask on the card, withdrawn by Ctrl+Alt+F12 before an answer:
/// denied unread; a second Ctrl+Alt+F12 gives `desktop` the screen back.
fn send_withdrawn(kind: usize, what: &'static str) -> Result<(), &'static str> {
    ask_on_card(kind)?;
    crate::claim_tests::type_keys(&crate::claim_tests::CTRL_ALT_F12);
    denied_unread(what)?;
    crate::claim_tests::type_keys(&crate::claim_tests::CTRL_ALT_F12);
    wait("desktop back on the screen", || {
        claim::showing() && ask_box::alive()
    })?;
    log!("desk-card: {what}: a send's card withdrawn was denied, no key read");
    Ok(())
}

/// With no card up, a send's ask is denied unread by `kind`.
fn send_with_no_card(kind: usize, what: &'static str) -> Result<(), &'static str> {
    ANSWER.store(0, Ordering::Release);
    threads::spawn("asker", asker, kind)?;
    denied_unread(what)?;
    log!("desk-card: {what}: a send with no card up was denied, no key read");
    Ok(())
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

/// Starts an ask of `kind` and waits until the card shows it.
fn ask_on_card(kind: usize) -> Result<(), &'static str> {
    ANSWER.store(0, Ordering::Release);
    threads::spawn("asker", asker, kind)?;
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
    // Y, twice once the card settled: the first arms it, the second runs it.
    ask_on_card(STOP)?;
    threads::sleep_ms(SETTLE_MS);
    crate::claim_tests::type_keys(&[0x15, 0x95]);
    threads::sleep_ms(KEY_MS);
    if ANSWER.load(Ordering::Acquire) != 0 {
        return Err("desk-card: one y ran the card");
    }
    crate::claim_tests::type_keys(&[0x15, 0x95]);
    if answered()? != code(Answer::Allow) || !claim::showing() {
        return Err("desk-card: y on the card did not allow, or the console took the screen");
    }
    log!("desk-card: an ask went to desktop's card, and y on it allowed");
    // A, Enter, then N.
    ask_on_card(STOP)?;
    threads::sleep_ms(SETTLE_MS);
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
    // A send's card withdrawn, by answer and by pick.
    send_withdrawn(SEND, "answer")?;
    send_withdrawn(SEND_PICKED, "pick")?;
    // desktop killed with a card up.
    ask_on_card(STOP)?;
    let &(pid, _) = process::named("desktop")
        .first()
        .ok_or("desk-card: desktop is gone")?;
    process::kill(pid, kabi::ExitReason::Killed).map_err(|_| "desk-card: cannot kill desktop")?;
    // Typed only once the console's menu takes keys: it drops those typed
    // before, and on TCG the asker may reach it well after the screen came back.
    wait("the ask back on the console", || {
        !claim::showing() && !ask_box::shown() && terminal::menu_takes_keys()
    })?;
    crate::claim_tests::type_keys(&[0x31, 0xB1]);
    if answered()? != code(Answer::Deny) {
        return Err("desk-card: the ask did not come back to the console");
    }
    log!("desk-card: with desktop killed, the ask came back and the console answered it");
    // No card up: a send is denied unread; another menu is the console's.
    send_with_no_card(SEND, "answer")?;
    send_with_no_card(SEND_PICKED, "pick")?;
    ANSWER.store(0, Ordering::Release);
    threads::spawn("asker", asker, STOP_PICKED)?;
    wait("pick's ask on the console", terminal::menu_takes_keys)?;
    crate::claim_tests::type_keys(&[0x31, 0xB1]);
    if answered()? != code(Answer::Deny) {
        return Err("desk-card: pick did not ask another menu on the console");
    }
    log!("desk-card: with no card up, pick asked another menu on the console");
    Ok(())
}
