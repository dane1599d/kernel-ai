//! `test=keep-running` (P4 task 7): Settings' keep-running switch, end to end.
//! Notes (`notes-window`) runs with a 3-second nap. A stand-in for the Settings
//! app runs as the plan line `settings-window` (this test plan only, never the
//! image's), and the case sends `desktop` its `settings put` requests as that
//! process, so the shell hears them with the kernel's name for the app.
//! - Boot 1: the supervisor refuses `keep` from anyone but the shell, and the
//!   shell refuses a change without a key or a click (user=0). With user=1 the
//!   shell sends `keep notes-window on` and saves `/system/state/keep`; Notes
//!   is then left alone past its nap and does not nap.
//! - Boot 2 (`reread`), on the same disk: the shell replays the saved list at
//!   start, so Notes again does not nap; `keep-running off` lets it nap.

use crate::display::claim;
use crate::{hpet, ipc, log, process, threads};
use alloc::format;
use alloc::string::String;

const APP: &str = "notes-window";
const SETTINGS: &str = "settings-window";
/// The plan lines this case adds; the stand-in only has to be alive.
pub const PLAN: &str = "desktop caps=display+input+serve pages=8192 stack_pages=512\n\
     notes-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1 nap_ms=3000\n\
     settings-window program=spinner pages=64 idle=1";
const WAIT_MS: u64 = 60_000;
const CALL_MS: u64 = 60_000;
/// Time for comphost to start the app and paint its first frame under TCG.
const OPEN_MS: u64 = 10_000;
/// The nap, and time for comphost to notice it is due.
const NAP_MS: u64 = 3_000 + 4_000;
/// How often the stand-in asks `desktop` for its answers.
const POLL_MS: u64 = 200;

fn wait(what: &'static str, mut done: impl FnMut() -> bool) -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        if done() {
            return Ok(());
        }
        threads::sleep_ms(50);
    }
    log!("keep-running: timed out waiting for {what}");
    Err(what)
}

fn pid_of(name: &str) -> Result<u64, &'static str> {
    process::named(name)
        .first()
        .map(|&(pid, _)| pid)
        .ok_or("keep-running: a service is missing")
}

/// `service`'s answer to `request`, sent as process `pid` would.
fn call_as(service: &str, pid: u64, request: &str) -> Result<String, &'static str> {
    let deadline = hpet::nanos() + CALL_MS * 1_000_000;
    ipc::call_as(service, request.as_bytes(), pid, deadline, || false)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "keep-running: a service did not reply")
}

/// Notes' `ctl status`, from the kernel: it ends `suspended` while it naps.
fn napping() -> Result<bool, &'static str> {
    ipc::call(APP, b"ctl status", CALL_MS)
        .map(|reply| {
            String::from_utf8_lossy(&reply)
                .trim_end()
                .ends_with(" suspended")
        })
        .map_err(|_| "keep-running: notes-window did not reply")
}

/// Uses Notes, so it is awake, then leaves it alone past its nap: whether it
/// napped.
fn naps_when_left() -> Result<bool, &'static str> {
    let count = "kai-call 1\nfunc app:notes/actions@1.0.0#count\nargs {}\n";
    let counted = ipc::call(APP, count.as_bytes(), CALL_MS)
        .is_ok_and(|reply| String::from_utf8_lossy(&reply).contains("{notes: "));
    if !counted {
        return Err("keep-running: notes-window did not answer a call");
    }
    threads::sleep_ms(NAP_MS);
    napping()
}

/// Sends `request` as the Settings app, with `user` as comphost would set it,
/// and waits for the shell's answer: `ok` or `err`, then its text.
fn settings(ticket: u32, user: bool, request: &str) -> Result<String, &'static str> {
    let pid = pid_of(SETTINGS)?;
    let put = format!("settings put\n{ticket}\t{}\t{request}", u8::from(user));
    if call_as("desktop", pid, &put)? != "ok" {
        return Err("keep-running: desktop refused the Settings app's request");
    }
    for _ in 0..WAIT_MS / POLL_MS {
        let answers = call_as("desktop", pid, "settings results")?;
        if let Some(answer) = answer_to(&answers, ticket) {
            return Ok(answer);
        }
        threads::sleep_ms(POLL_MS);
    }
    Err("keep-running: the shell never answered the Settings app")
}

/// `ticket`'s answer among `answers` (`<ticket>\tok|err\t<len>\n<text>` each),
/// as `ok <text>` or `err <text>`.
fn answer_to(mut answers: &str, ticket: u32) -> Option<String> {
    while !answers.is_empty() {
        let (head, rest) = answers.split_once('\n')?;
        let mut fields = head.split('\t');
        let (number, verdict, len) = (fields.next()?, fields.next()?, fields.next()?);
        let len: usize = len.parse().ok()?;
        let text = rest.get(..len)?;
        if number.parse() == Ok(ticket) {
            return Some(format!("{verdict} {text}"));
        }
        answers = &rest[len..];
    }
    None
}

fn up() -> Result<(), &'static str> {
    wait(
        "desktop, the shell, Notes and the Settings stand-in",
        || {
            claim::showing()
                && [APP, SETTINGS, "shell"]
                    .iter()
                    .all(|name| !process::named(name).is_empty())
        },
    )?;
    threads::sleep_ms(OPEN_MS);
    Ok(())
}

pub fn check(reread: bool) -> Result<(), &'static str> {
    up()?;
    if reread {
        return check_replayed();
    }
    let refused = call_as("supervisor", pid_of(SETTINGS)?, "keep notes-window on")?;
    log!("keep-running: keep from the Settings app itself: {refused}");
    if refused != "err only the shell keeps apps running" {
        return Err("keep-running: the supervisor took keep from an app");
    }
    let unclicked = settings(1, false, "keep-running on notes-window")?;
    log!("keep-running: user=0: {unclicked}");
    if !unclicked.starts_with("err ") {
        return Err("keep-running: a change without a key or a click went through");
    }
    if !naps_when_left()? {
        return Err("keep-running: notes-window did not nap before it was kept");
    }
    let kept = settings(2, true, "keep-running on notes-window")?;
    log!("keep-running: user=1: {kept}");
    if kept != "ok notes-window keep-running on" {
        return Err("keep-running: the shell did not keep notes-window running");
    }
    if naps_when_left()? {
        return Err("keep-running: a kept app napped");
    }
    log!("keep-running: boot 1 kept notes-window running from a Settings click, shell only");
    Ok(())
}

fn check_replayed() -> Result<(), &'static str> {
    if naps_when_left()? {
        return Err("keep-running: boot 2 did not keep notes-window running");
    }
    log!("keep-running: boot 2 kept notes-window running from /system/state/keep");
    let off = settings(1, true, "keep-running off notes-window")?;
    log!("keep-running: user=1: {off}");
    if off != "ok notes-window keep-running off" {
        return Err("keep-running: the shell did not let notes-window go");
    }
    if !naps_when_left()? {
        return Err("keep-running: an app let go did not nap");
    }
    log!("keep-running: boot 2 let notes-window go, and it napped");
    Ok(())
}
