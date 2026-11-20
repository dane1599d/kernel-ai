//! `test=desk-sends` (ADR 0005 §7, "Amended 2026-11-19"): a send typed in
//! the Kai pane at 1920x1080, end to end through the desk card. xtask boots
//! it on virtio-gpu at that size; the mock model answers [`LINE`] with a
//! `net_send` of [`text`] (`xtask/src/mock_llm/sends.rs`).
//! - The card shows only the call code made, `net_send("hooks.example",
//!   "/hook", "<16 hex of the text's SHA-256>")`, never the text, which
//!   `desktop` shows in the Kai pane beside it (its lines, which xtask checks).
//! - The text is longer than the pane shows, so `y`, twice past the card's
//!   settle time, does not allow it before it was read through. PageDown
//!   scrolls it to the end (`desktop` logs that its whole text has been on the
//!   screen), and then `y` twice allows it.
//! - The Allow holds the send 5 s: the shell's log holds "Sending to
//!   hooks.example in 5 s. undo <id>". `undo <id>`, a line to the shell,
//!   cancels it, and once the hold is over the log says it was not sent.
//!
//! An agent's send is not covered here: no boot plan runs an agent (ADR
//! 0013's agents are wired into nothing yet), and a desk line's asker is the
//! pane or a window, both the person's (`shellcore::deskask::Asker`).

use crate::claim_tests::{type_keys, type_text};
use crate::display::claim;
use crate::{ask_box, ipc, log, terminal, threads};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const CASE: &str = "desk-sends";
/// The line typed in the Kai pane: it asks for a send and names its host.
pub(crate) const LINE: &str = "send my long note to hooks.example";
/// The text's numbered sentences, on one line: a send's text holds no
/// control character (`termcore::once::quote`, so no line break) and at most
/// `agentabi::SEND_MAX` characters. Soft-wrapped, it is more rows than the
/// pane shows at 1920x1080, so it scrolls. `xtask/src/mock_llm/sends.rs`
/// makes the same text, which its test checks.
const TEXT_PIECES: usize = 150;
const HOST: &str = "hooks.example";
/// `agentabi::HOLD_MS`: an allowed send waits this long for `undo`.
const HOLD_MS: u64 = 5_000;
const WAIT_MS: u64 = 60_000;
const CALL_MS: u64 = 30_000;
/// Time for a key to reach `desktop` and its frame to show.
const KEY_MS: u64 = 1_000;
/// Past the card's settle time, in which it takes no key (#864).
const SETTLE_MS: u64 = termcore::menu::SETTLE_MS + 300;
/// Time for the terminal to reach its prompt and for llm to come up.
const PROMPT_MS: u64 = 4_000;
/// Ctrl down, Space down and up, Ctrl up: the pane takes the keys.
const CTRL_SPACE: [u8; 4] = [0x1D, 0x39, 0xB9, 0x9D];
/// `y` down and up: a card's first arms it, the second runs it.
const YES: [u8; 2] = [0x15, 0x95];
/// PageDown down and up: set 1's `E0 51`, HID usage 0x4E (`kabi::input`).
const PAGE_DOWN: [u8; 4] = [0xE0, 0x51, 0xE0, 0xD1];
/// Most PageDowns before the case gives up: the text is read through long
/// before, at any pane of 3 rows or more.
const MAX_PAGES: usize = 50;
/// What the Allow's hold says in the shell's log, before its entry.
const HELD: &str = "Sending to hooks.example in 5 s. undo ";
/// What a send `undo` cancelled comes to (`shellcore::outbox::ended_line`).
const UNDONE: &str = "Not sent to hooks.example: undone.";

/// The text the mock model sends: numbered sentences on one line.
pub(crate) fn text() -> String {
    let pieces: Vec<String> = (1..=TEXT_PIECES)
        .map(|n| format!("{n:03} the text goes on."))
        .collect();
    pieces.join(" ")
}

/// The call the card shows: the host, the path and the text's digest.
fn card_call(text: &str) -> String {
    let mut sha = termcore::once::Sha256::new();
    sha.update(text.as_bytes());
    let hex: String = sha.finish()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("net_send(\"{HOST}\", \"/hook\", \"{hex}\")")
}

fn wait(what: &'static str, mut done: impl FnMut() -> bool) -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 100 {
        if done() {
            return Ok(());
        }
        threads::sleep_ms(100);
    }
    log!("{CASE}: timed out waiting for {what}");
    Err(what)
}

/// The shell's reply to `line`, sent as the console's line.
fn shell(line: &str) -> Result<String, &'static str> {
    ipc::call("shell", line.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "desk-sends: the shell did not reply")
}

/// The shell's action log, as `/system/log` holds it. A line to the shell:
/// while the card is up it would discard the send it asks about (any line
/// but the answer does, `Shell::eval_line`), so the case reads it only
/// after the card has answered.
fn actlog() -> String {
    shell("log actlog").unwrap_or_default()
}

/// The entry the hold names for its `undo`, once the log holds it.
fn held_entry(log: &str) -> Option<u64> {
    let (_, rest) = log.split_once(HELD)?;
    let digits = rest.split(|c: char| !c.is_ascii_digit()).next()?;
    digits.parse().ok()
}

/// The line typed in the pane goes to the card, which shows the call alone.
fn ask_from_the_pane() -> Result<(), &'static str> {
    type_keys(&CTRL_SPACE);
    threads::sleep_ms(KEY_MS);
    type_text(LINE);
    type_text("\n");
    // At 1920x1080 the card sits beside the open pane, so the send is the
    // card's; asked at the console instead, `desk_send_fits` said no.
    wait("the send on the card or the console", || {
        ask_box::shown() || terminal::menu_takes_keys()
    })?;
    if !ask_box::shown() {
        return Err("desk-sends: desk_send_fits said no at 1920x1080, and the console asked");
    }
    let question = ask_box::question().unwrap_or_default();
    log!("{CASE}: the card asks: {question}");
    if !question.contains(&card_call(&text())) || question.contains("the text goes on") {
        return Err("desk-sends: the card did not show the call with the digest alone");
    }
    log!("{CASE}: the card shows only the call, with the text's digest");
    Ok(())
}

/// `y`, twice past the settle time, before the text was read through.
fn yes_before_reading() -> Result<(), &'static str> {
    threads::sleep_ms(SETTLE_MS);
    type_keys(&YES);
    threads::sleep_ms(KEY_MS);
    type_keys(&YES);
    threads::sleep_ms(KEY_MS * 2);
    // The card still up is the send not allowed: the shell holds it only
    // on the card's answer. No log read here (see `actlog`).
    if !ask_box::shown() {
        return Err("desk-sends: y allowed the send before its text was read through");
    }
    log!("{CASE}: y before the text was read through did not allow it");
    Ok(())
}

/// A page at a time, `y` twice after each: the card answers only once the
/// last page has been on the screen, however many rows the pane shows. The
/// Allow holds the send.
fn read_through_and_allow() -> Result<u64, &'static str> {
    let mut pages = 0;
    while ask_box::shown() {
        if pages == MAX_PAGES {
            return Err("desk-sends: the card never allowed, the text read to its end");
        }
        type_keys(&PAGE_DOWN);
        pages += 1;
        threads::sleep_ms(KEY_MS);
        type_keys(&YES);
        threads::sleep_ms(KEY_MS);
        type_keys(&YES);
        threads::sleep_ms(KEY_MS);
    }
    log!("{CASE}: the card allowed after {pages} PageDown(s), once the text was read through");
    let mut entry = None;
    wait("the hold in the shell's log", || {
        entry = held_entry(&actlog());
        entry.is_some()
    })?;
    let entry = entry.ok_or("desk-sends: the hold named no entry")?;
    log!("{CASE}: read through, y allowed the send, held 5 s with undo {entry}");
    Ok(entry)
}

/// `undo <entry>` within the hold: the send never goes.
fn undo_within_the_hold(entry: u64) -> Result<(), &'static str> {
    let undone = shell(&format!("undo {entry}"))?;
    log!("{CASE}: undo {entry}: {undone}");
    if !undone.contains("the send to hooks.example, not sent") {
        return Err("desk-sends: undo did not take the send back");
    }
    threads::sleep_ms(HOLD_MS + 2_000);
    let log = actlog();
    if !log.contains(UNDONE) || log.contains("Sent to hooks.example") {
        return Err("desk-sends: the send went, or its log does not say it was undone");
    }
    log!("{CASE}: undo within the hold cancelled the send, and it was not sent");
    Ok(())
}

pub fn check() -> Result<(), &'static str> {
    threads::spawn("terminal", terminal::run, 0)?;
    wait("desktop, its trusted layer and the shell", || {
        claim::showing() && ask_box::alive() && ipc::server_of("shell").is_some()
    })?;
    threads::sleep_ms(PROMPT_MS);
    ask_from_the_pane()?;
    yes_before_reading()?;
    let entry = read_through_and_allow()?;
    undo_within_the_hold(entry)
}
