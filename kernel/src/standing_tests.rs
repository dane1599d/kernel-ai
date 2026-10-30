//! `test=desk-standing` (P3 task 11): a standing instruction fired by a window
//! event. Notes (`notes-window`, a comphost app with a window) holds a note.
//! - The user types `rules_add on minimize(notes-window) ->
//!   app_suspend(notes-window) until restore(notes-window)`; the shell asks on
//!   `desktop`'s card, and `y` allows it. The kernel notes the rule and gives
//!   the shell its handle, so the shell holds a rule covering
//!   `app_suspend("notes-window")` and its inverse, and nothing else.
//! - Notes' minimize control is clicked: `desktop` posts `minimize
//!   notes-window` to the event box. The shell takes it between lines and fires
//!   the rule: `suspend notes-window rule` to the supervisor, which checks the
//!   rule with RULE_CHECK, and the kernel sends Notes `ctl suspend`, freeing
//!   its instance. The firing is the shell's own log entry. The same request
//!   from a process holding no rule is refused.
//! - Alt+Tab restores the window: `desktop` posts `restore notes-window`, the
//!   firing's `until` resumes Notes the same way, and its note is still there.

use crate::display::claim;
use crate::pointer::{self, Event, Motion, Source};
use crate::{ask_box, handle, hpet, ipc, log, process, terminal, threads};
use alloc::string::String;
use core::sync::atomic::{AtomicBool, Ordering};

/// The plan lines this case adds.
pub const PLAN: &str = "desktop caps=display+input+serve+ask pages=8192 stack_pages=512\n\
     notes-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1";
const APP: &str = "notes-window";
const RULE: &str =
    "rules_add on minimize(notes-window) -> app_suspend(notes-window) until restore(notes-window)";
const WAIT_MS: u64 = 60_000;
/// Time for `desktop` to take each pointer step in.
const STEP_MS: u64 = 400;
/// Time for comphost to start the app and paint its first frame under TCG.
const OPEN_MS: u64 = 10_000;
/// A call to Notes or the supervisor: a suspend saves the app's state.
const CALL_MS: u64 = 60_000;
/// Where `desktop` starts the pointer: the middle of 1280x800.
const START: (i32, i32) = (640, 400);
/// The minimize control of the window at the first cascade spot (48,52, frame
/// 642x454, as `ui-window` finds it): the third control from the right, 76 px
/// in from the frame's right end, and 17 px down from its top.
const MINIMIZE: (i32, i32) = (48 + 642 - 76, 52 + 17);
/// Alt down, Tab down and up, Alt up (scan code set 1).
const ALT_TAB: [u8; 4] = [0x38, 0x0F, 0x8F, 0xB8];
/// `y` down and up.
const YES: [u8; 2] = [0x15, 0x95];

static ASKED: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);

fn wait(what: &'static str, mut done: impl FnMut() -> bool) -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        if done() {
            return Ok(());
        }
        threads::sleep_ms(50);
    }
    log!("desk-standing: timed out waiting for {what}");
    Err(what)
}

/// A USB-style relative report with `buttons` held.
fn report(dx: i32, dy: i32, buttons: u8) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        pointer::push(Event {
            source: Source::UsbMouse,
            motion: Motion::Relative { dx, dy },
            buttons,
        });
        crate::input::wake_pending();
    });
    threads::sleep_ms(STEP_MS);
}

fn notes(request: &str) -> Result<String, &'static str> {
    ipc::call(APP, request.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "desk-standing: notes did not reply")
}

/// Types the rule as the user would, on a thread of its own as the terminal's.
fn liner(_: usize) {
    ASKED.store(terminal::shell_line_answered(RULE), Ordering::Release);
    DONE.store(true, Ordering::Release);
}

fn pid_of(name: &str) -> Result<u64, &'static str> {
    process::named(name)
        .first()
        .map(|&(pid, _)| pid)
        .ok_or("desk-standing: a service is missing")
}

fn slot_of(pid: u64) -> Option<usize> {
    process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid))
}

/// Waits for the shell's action log to hold a done firing of rule 1 whose
/// script is `script`: the shell took the event and fired by itself.
fn fired(script: &'static str) -> Result<(), &'static str> {
    wait(script, || {
        let Ok(log) = ipc::call("shell", b"log actlog", CALL_MS) else {
            return false;
        };
        let log = String::from_utf8_lossy(&log);
        // An entry's E line, then its R and O, then its C with the script.
        log.split("\nE ").any(|entry| {
            entry.contains("form=standing tier=run consent=standing:1 state=done")
                && entry.contains(script)
        })
    })
}

/// The supervisor's answer to `request`, sent as process `pid` would.
fn supervisor_as(pid: u64, request: &str) -> Result<String, &'static str> {
    let deadline = hpet::nanos() + CALL_MS * 1_000_000;
    ipc::call_as("supervisor", request.as_bytes(), pid, deadline, || false)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "desk-standing: the supervisor did not reply")
}

pub fn check() -> Result<(), &'static str> {
    wait("desktop, its trusted layer and Notes", || {
        claim::showing() && ask_box::alive() && !process::named(APP).is_empty()
    })?;
    threads::sleep_ms(OPEN_MS);
    let add = "kai-call 1\nfunc app:notes/actions@1.0.0#add\nargs {text: \"buy milk\"}\n";
    if !notes(add)?.contains("ok(1)") {
        return Err("desk-standing: the note was not added");
    }
    // The rule, allowed on the card.
    threads::spawn("liner", liner, 0)?;
    wait("the rule's ask on the card", ask_box::shown)?;
    threads::sleep_ms(1_000);
    crate::claim_tests::type_keys(&YES);
    wait("the shell's answer", || DONE.load(Ordering::Acquire))?;
    let shell = pid_of("shell")?;
    let held = |call: &str| slot_of(shell).is_some_and(|slot| handle::rules::covers(slot, call));
    if !ASKED.load(Ordering::Acquire)
        || !held("app_suspend(\"notes-window\")")
        || !held("app_resume(\"notes-window\")")
        || held("app_suspend(\"shell\")")
        || held("service_stop(\"notes-window\")")
    {
        return Err("desk-standing: the Allow did not give the shell exactly the rule");
    }
    log!("desk-standing: the rule was allowed on the card, and the shell holds it");
    // Minimize: the event, which the shell takes, then its firing.
    report(MINIMIZE.0 - START.0, MINIMIZE.1 - START.1, 0);
    report(0, 0, 1);
    report(0, 0, 0);
    fired("\napp_suspend(\"notes-window\")")?;
    let desktop = pid_of("desktop")?;
    let refused = supervisor_as(desktop, "suspend notes-window rule")?;
    if !refused.starts_with("err rule") {
        log!("desk-standing: without the rule: {refused}");
        return Err("desk-standing: a process without the rule suspended Notes");
    }
    log!("desk-standing: minimizing Notes fired the rule, and Notes is suspended");
    // Restore: the event, then the "until", which the shell fires as well.
    threads::sleep_ms(1_000);
    crate::claim_tests::type_keys(&ALT_TAB);
    fired("undo app_suspend(\"notes-window\")")?;
    let count = "kai-call 1\nfunc app:notes/actions@1.0.0#count\nargs {}\n";
    if !notes(count)?.contains("{notes: 1, words: 2}") {
        return Err("desk-standing: the note did not come back");
    }
    log!("desk-standing: restoring Notes resumed it, with its note intact");
    Ok(())
}
