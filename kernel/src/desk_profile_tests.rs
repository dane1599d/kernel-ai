//! `test=desk-profile` (P3's exit case): a resource profile applied exactly,
//! switched by a real focus change, and undone.
//! - The user's rule `on focus(notes-window) -> profile_set(gaming)` is
//!   allowed on `desktop`'s card.
//! - Ctrl+Space takes the focus to the panel, and a click on Notes' title bar
//!   brings it back: `desktop` posts `focus notes-window`, and the shell fires
//!   the rule.
//! - `gaming` applies exactly as `kaiprofile` states it: every app but the
//!   focused one, here `spin-a` and `spin-b`, goes to the background with its
//!   quota unchanged, and the system's services (the shell, the supervisor,
//!   `desktop`, Notes' service) keep their class and quota.
//! - `undo` of the firing puts `default` back: both spinners normal again.

use crate::desk_tests::report;
use crate::display::claim;
use crate::{ask_box, ipc, log, process, sched_control, terminal, threads};
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The plan lines this case adds: `desktop`, Notes' window, and two apps the
/// supervisor starts and does not keep.
pub const PLAN: &str = "desktop caps=display+input+serve+ask pages=8192 stack_pages=512\n\
     notes-window program=comphost caps=serve+component+call uses=desktop pages=8192 stack_pages=512 burst_ms=20000 version=1\n\
     spin-a program=spinner burst_ms=3600000 unsupervised\n\
     spin-b program=spinner burst_ms=3600000 unsupervised";
const RULE: &str = "rules_add on focus(notes-window) -> profile_set(gaming)";
/// The firing's script, as the log has it.
const GAMING: &str = "profile_set(\"gaming\")";
const APPS: [&str; 2] = ["spin-a", "spin-b"];
const SYSTEM: [&str; 4] = ["shell", "supervisor", "desktop", "notes-window"];
const WAIT_MS: u64 = 60_000;
const CALL_MS: u64 = 30_000;
/// Time for comphost to start Notes and paint its first frame under TCG.
const OPEN_MS: u64 = 10_000;
/// Where `desktop` starts the pointer: the middle of 1280x800.
const START: (i32, i32) = (640, 400);
/// Notes' title bar, clear of its controls: its frame is at 48,52.
const NOTES_TITLE: (i32, i32) = (300, 68);
/// Ctrl down, Space down and up, Ctrl up; `y` down and up (scan code set 1).
const CTRL_SPACE: [u8; 4] = [0x1D, 0x39, 0xB9, 0x9D];
const YES: [u8; 2] = [0x15, 0x95];

static ASKED: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);
static UNDONE: AtomicBool = AtomicBool::new(false);
/// The log entry `undoer` takes back.
static UNDO_ID: AtomicU64 = AtomicU64::new(0);

fn wait(what: &'static str, mut done: impl FnMut() -> bool) -> Result<(), &'static str> {
    for _ in 0..WAIT_MS / 50 {
        if done() {
            return Ok(());
        }
        threads::sleep_ms(50);
    }
    log!("desk-profile: timed out waiting for {what}");
    Err(what)
}

/// Types the rule as the user would.
fn liner(_: usize) {
    ASKED.store(terminal::shell_line_answered(RULE), Ordering::Release);
    DONE.store(true, Ordering::Release);
}

fn undoer(_: usize) {
    let id = UNDO_ID.load(Ordering::Acquire);
    terminal::shell_line_answered(&alloc::format!("undo #{id}"));
    UNDONE.store(true, Ordering::Release);
}

/// Each named process's pid, quota and whether it is background.
fn settings(names: &[&str]) -> Option<Vec<(u64, u64, bool)>> {
    names
        .iter()
        .map(|name| {
            process::with_table(|t| {
                t.iter()
                    .find(|p| p.live() && p.name.as_str() == *name)
                    .map(|p| (p.pid, p.limits.max_pages))
            })
            .map(|(pid, pages)| (pid, pages, sched_control::is_background(pid)))
        })
        .collect()
}

/// The ids of the shell's log entries for done firings of rule 1 with
/// `script`, oldest first.
fn firings(script: &str) -> Vec<u64> {
    let Ok(log) = ipc::call("shell", b"log actlog", CALL_MS) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&log)
        .split("\nE ")
        .filter(|entry| {
            entry.contains("form=standing tier=run consent=standing:1 state=done")
                && entry.contains(script)
        })
        .filter_map(|entry| entry.split_whitespace().next()?.parse().ok())
        .collect()
}

pub fn check() -> Result<(), &'static str> {
    wait("desktop, its trusted layer, Notes and the apps", || {
        claim::showing()
            && ask_box::alive()
            && settings(&SYSTEM).is_some()
            && settings(&APPS).is_some()
    })?;
    threads::sleep_ms(OPEN_MS);
    let (apps, system) = (
        settings(&APPS).ok_or("desk-profile: an app is gone")?,
        settings(&SYSTEM).ok_or("desk-profile: a service is gone")?,
    );
    if apps
        .iter()
        .chain(&system)
        .any(|&(.., background)| background)
    {
        return Err("desk-profile: something started in the background");
    }
    // The rule, allowed on the card.
    threads::spawn("liner", liner, 0)?;
    wait("the rule's ask on the card", ask_box::shown)?;
    threads::sleep_ms(1_000);
    crate::claim_tests::type_keys(&YES);
    wait("the shell's answer", || DONE.load(Ordering::Acquire))?;
    if !ASKED.load(Ordering::Acquire) {
        return Err("desk-profile: the rule was made without an ask");
    }
    log!("desk-profile: the focus rule was allowed on the card");
    // A real focus change: to the panel, then back to Notes.
    crate::claim_tests::type_keys(&CTRL_SPACE);
    threads::sleep_ms(1_000);
    report(NOTES_TITLE.0 - START.0, NOTES_TITLE.1 - START.1, 0);
    report(0, 0, 1);
    report(0, 0, 0);
    wait("the rule's firing", || !firings(GAMING).is_empty())?;
    // Exactly gaming: the apps back, quotas kept, the system untouched.
    let after = settings(&APPS).ok_or("desk-profile: an app is gone")?;
    let kept = settings(&SYSTEM).ok_or("desk-profile: a service is gone")?;
    let quotas = |list: &[(u64, u64, bool)]| list.iter().map(|s| s.1).collect::<Vec<_>>();
    if !after.iter().all(|&(.., background)| background) || quotas(&after) != quotas(&apps) {
        return Err("desk-profile: gaming did not send the apps back, quotas kept");
    }
    if kept != system {
        return Err("desk-profile: gaming changed a system service");
    }
    log!(
        "desk-profile: focusing Notes switched to gaming: both apps went to the background, their quotas and the system untouched"
    );
    // Undo puts default back. The rule may have fired more than once (the
    // focus coming back from the card fires it too), so each firing is undone
    // by its id, newest first, until `default` is in force: a bare `undo`
    // after the first would undo that undo, never the firing before it.
    // TODO(#524): once a rule fires only on events posted after it was made,
    // one `undo` is enough; drop this loop.
    for id in firings(GAMING).into_iter().rev() {
        UNDO_ID.store(id, Ordering::Release);
        UNDONE.store(false, Ordering::Release);
        threads::spawn("undoer", undoer, 0)?;
        wait("the undo", || UNDONE.load(Ordering::Acquire))?;
        if settings(&APPS).as_deref() == Some(apps.as_slice()) {
            break;
        }
    }
    if settings(&APPS).as_deref() != Some(apps.as_slice()) {
        return Err("desk-profile: undo did not put the apps back as they were");
    }
    log!("desk-profile: undo put default back: both apps normal again");
    Ok(())
}
