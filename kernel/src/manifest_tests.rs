//! M9d self-tests, on refs xtask prepared in a snapshot repository. In each, notes
//! starts on the good version (or 2 when there is none) and the kernel's own thread
//! swaps it to the pending one without a yes. `test=manifest`: three calls end
//! probation with a commit. `test=manifest-good`: `count` traps on the pending
//! version and it rolls back. Git must acknowledge either; the cases' host checks
//! then read the good and failed refs.

use crate::process::components;
use crate::{boot_manifest, hpet, ipc, log, threads};
use alloc::format;
use alloc::string::String;
use livecore::call::REPLY_HEADER;
use livecore::ledger::Ledger;

const NAME: &str = "notes";
const WAIT_MS: u64 = 120_000;
const CALL_MS: u64 = 60_000;

fn ledger() -> Ledger {
    components::ledger(NAME).unwrap_or_default()
}

/// Waits until `done` holds for notes's ledger; the ledger then.
fn wait_until(what: &str, done: impl Fn(&Ledger) -> bool) -> Result<Ledger, String> {
    for _ in 0..WAIT_MS / 10 {
        let ledger = ledger();
        if done(&ledger) {
            return Ok(ledger);
        }
        threads::sleep_ms(10);
    }
    Err(format!("manifest: no {what} within {WAIT_MS} ms"))
}

fn call(request: &str) -> Result<String, String> {
    ipc::call(NAME, request.as_bytes(), CALL_MS)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|error| format!("manifest: {request:?} got no reply: {error:?}"))
}

/// Waits until git acknowledged every job in the outbox.
fn settle_sync() -> Result<(), String> {
    let deadline = hpet::nanos() + WAIT_MS * 1_000_000;
    while components::next_sync().is_some() {
        if hpet::nanos() > deadline {
            return Err(format!(
                "manifest: git did not acknowledge within {WAIT_MS} ms"
            ));
        }
        threads::sleep_ms(50);
    }
    Ok(())
}

/// notes's swap to `pending` began, by the kernel's own thread, from `start`.
fn swapped_from(start: &str, pending: &str) -> Result<(), String> {
    let ledger = wait_until("swap", |l| {
        l.candidate() == Some(pending) || l.is_bad(pending).is_some() || l.active() == Some(pending)
    })?;
    if ledger.candidate() != Some(pending) {
        return Err(format!(
            "manifest: the swap to {pending} did not reach probation ({})",
            ledger.last_detail
        ));
    }
    let started = ledger.lineage.front().map(String::as_str);
    if started != Some(start) {
        return Err(format!(
            "manifest: notes started on {started:?}, not {start}"
        ));
    }
    let mut reply = boot_manifest::swap_reply();
    for _ in 0..WAIT_MS / 10 {
        if reply.is_some() {
            break;
        }
        threads::sleep_ms(10);
        reply = boot_manifest::swap_reply();
    }
    let reply = reply.unwrap_or_default();
    if !reply.starts_with(&format!("ok probation {pending} ")) {
        return Err(format!("manifest: the swap was answered {reply:?}"));
    }
    Ok(())
}

/// The manifest's labels, which the test needs: the pending one, and the good
/// one when `good` says the case has one.
fn labels(good: bool) -> Result<(String, String), String> {
    match boot_manifest::labels() {
        (Some(label), Some(pending)) if good => Ok((label, pending)),
        (None, Some(pending)) if !good => Ok((String::from("2"), pending)),
        labels => Err(format!("manifest: the boot manifest is {labels:?}")),
    }
}

/// `test=manifest`: three calls, and the pending version is committed.
fn commits() -> Result<String, String> {
    let (start, pending) = labels(false)?;
    swapped_from(&start, &pending)?;
    for _ in 0..3 {
        let reply = call(livecore::notes_call!("about", "{}"))?;
        let about = format!("{REPLY_HEADER}\nresult \"notes {pending} ");
        if !reply.starts_with(&about) || !reply.contains("edited live") {
            return Err(format!("manifest: about answered {reply:?}"));
        }
    }
    let ledger = ledger();
    if ledger.active() != Some(pending.as_str()) || ledger.counters.commits != 1 {
        return Err(format!(
            "manifest: notes runs {:?} after {} commits",
            ledger.active(),
            ledger.counters.commits
        ));
    }
    Ok(format!(
        "notes started on {start}, swapped to {pending} without a yes and committed it"
    ))
}

/// `test=manifest-good`: `count` traps on the pending version, which rolls back
/// to the good one.
fn rolls_back() -> Result<String, String> {
    let (good, pending) = labels(true)?;
    swapped_from(&good, &pending)?;
    let reply = call(livecore::notes_call!("count", "{}"))?;
    let ledger = ledger();
    let trapped = ledger.is_bad(&pending) == Some(livecore::Reason::Trap);
    let counted = reply.starts_with(&format!("{REPLY_HEADER}\nresult {{notes: "));
    if !counted || ledger.active() != Some(good.as_str()) || !trapped {
        return Err(format!(
            "manifest: count answered {reply:?}; notes runs {:?} ({})",
            ledger.active(),
            ledger.last_detail
        ));
    }
    Ok(format!(
        "notes started on {good}, swapped to {pending}, which trapped and rolled back"
    ))
}

fn run(good: bool) -> Result<(), String> {
    let done = if good { rolls_back()? } else { commits()? };
    settle_sync()?;
    let shown = boot_manifest::lines();
    for line in &shown {
        log!("{line}");
    }
    let (_, pending) = boot_manifest::labels();
    if !shown[0].ends_with(&format!("pending {}", pending.unwrap_or_default())) {
        return Err(String::from(
            "manifest: :boot does not show the pending version",
        ));
    }
    log!("manifest: {done}");
    Ok(())
}

/// `good` says whether the boot has a good version, whose swap then traps.
fn check_case(good: bool) -> Result<(), &'static str> {
    if crate::build_service::target().is_none() {
        return Err("manifest: boot with build= and build_token=");
    }
    if let Err(why) = crate::live_tests::wait_for_host(0) {
        log!("{why}");
        return Err("manifest: the notes host never came up");
    }
    run(good).map_err(|why| {
        log!("{why}");
        "manifest: the boot manifest scenario failed"
    })
}

pub fn check() -> Result<(), &'static str> {
    check_case(false)
}

pub fn check_good() -> Result<(), &'static str> {
    check_case(true)
}
