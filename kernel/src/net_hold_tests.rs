//! `net-hold` (P5 close, ADR 0002 D2's `Hold`): a send the person allows waits
//! 5 s with `undo` live, then goes once; one undone in its hold never goes;
//! and one to a site the line does not name is refused. The mock model calls
//! `net_send` for each line (xtask's `mock_llm`), and the HTTPS mock records
//! every POST, which the case's host side counts afterwards: exactly one,
//! `hello` to `/hook` (`cases::netrows`).

use crate::{allow, ipc, log, threads};
use alloc::format;
use alloc::string::String;
use termcore::menu::{self, Answer};

/// How long a line may take: the mock model's answer and the trusted path.
const LINE_TIMEOUT_MS: u64 = 30_000;
/// Well past a send's 5 s hold and the OS loop's pass after it.
const PAST_HOLD_MS: u64 = 12_000;
/// Within a send's hold: `undo` still cancels it.
const WITHIN_HOLD_MS: u64 = 1_000;
/// Well into a send's hold, still short of its 5 s: it must still be held.
const STILL_HELD_MS: u64 = 3_000;

/// The line whose send goes, and the one whose send is undone.
const SENT: &str = "send hello to updates.kai.test/hook";
const UNDONE: &str = "send bye to updates.kai.test/hook";
/// A line that names no site: the mock's send to the mock host is refused.
const UNNAMED: &str = "send hi to my webhook";

fn reply_to(line: &str) -> Result<String, &'static str> {
    let reply =
        ipc::call("shell", line.as_bytes(), LINE_TIMEOUT_MS).map_err(|_| "net-hold: no reply")?;
    Ok(String::from_utf8_lossy(&reply).into_owned())
}

/// `line`'s card, allowed through the trusted path: the shell's reply.
fn allowed(line: &str) -> Result<String, &'static str> {
    let reply = reply_to(line)?;
    let Some(Ok(shown)) = menu::split_shown(&reply) else {
        log!("net-hold: {line:?} answered {reply:?}");
        return Err("net-hold: a send did not show its text above the menu");
    };
    // Either wording: "... this text to HOST:", or (#1062) "... this text (N
    // bytes) to HOST:".
    if !(shown.header.starts_with("Kai wants to send this text")
        && shown.header.ends_with(" to updates.kai.test:"))
    {
        return Err("net-hold: the text to send was not shown under code's header");
    }
    let menu = menu::Menu::decode(shown.menu).map_err(|_| {
        log!("net-hold: {line:?} answered {reply:?}");
        "net-hold: a send did not ask on the menu"
    })?;
    if !menu.question().contains("It waits 5 s for undo") {
        log!("net-hold: the card said {:?}", menu.question());
        return Err("net-hold: the card did not say the send waits for undo");
    }
    let granted = allow::allow(menu.id, menu.call(), Answer::Allow)
        .ok()
        .flatten()
        .ok_or("net-hold: the trusted path could not mint a ONCE")?;
    let reply = reply_to(&menu::granted_line(menu.id, Answer::Allow, granted.granted))?;
    allow::end(&granted);
    Ok(reply)
}

/// What the action log says of the latest send: its change's `after=` word.
fn last_send_after() -> Result<String, &'static str> {
    let log = reply_to("log actlog")?;
    let after = log
        .lines()
        .rev()
        .find(|line| line.contains("net_send owner=fetch"))
        .and_then(|line| line.split(" after=").nth(1))
        .and_then(|rest| rest.split(' ').next())
        .ok_or("net-hold: the log holds no send")?;
    Ok(String::from(after))
}

pub fn check() -> Result<(), &'static str> {
    // A: allowed, held, then sent once.
    let held = allowed(SENT)?;
    if !held.contains("undo") {
        log!("net-hold: the Allow answered {held:?}");
        return Err("net-hold: an allowed send was not held with its undo");
    }
    threads::sleep_ms(STILL_HELD_MS);
    if last_send_after()? != "held" {
        return Err("net-hold: an allowed send was not still held 3 s in");
    }
    threads::sleep_ms(PAST_HOLD_MS);
    if last_send_after()? != "sent" {
        return Err("net-hold: an allowed send was not sent after its hold");
    }
    // B: allowed, then undone within its hold: never sent.
    let held = allowed(UNDONE)?;
    let id = held
        .split("undo ")
        .nth(1)
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
        .filter(|n| !n.is_empty())
        .ok_or("net-hold: the held line named no entry to undo")?;
    threads::sleep_ms(WITHIN_HOLD_MS);
    let undone = reply_to(&format!("undo {id}"))?;
    if !undone.contains("not sent") {
        log!("net-hold: undo {id} answered {undone:?}");
        return Err("net-hold: undo within the hold did not cancel the send");
    }
    threads::sleep_ms(PAST_HOLD_MS);
    if last_send_after()? != "cancelled" {
        return Err("net-hold: an undone send was not logged cancelled");
    }
    // C: a site the line does not name is refused, with no card.
    let refused = reply_to(UNNAMED)?;
    if !refused.contains("a send goes only to a site the request names") {
        log!("net-hold: {UNNAMED:?} answered {refused:?}");
        return Err("net-hold: a send to a site the line did not name was not refused");
    }
    log!(
        "net-hold: an allowed send waited 5 s and went once, one undone in its hold never went, and one to a site the line did not name was refused"
    );
    Ok(())
}
