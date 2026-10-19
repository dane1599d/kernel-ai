//! M9d self-test (`test=manifest`): the boot manifest, on refs xtask prepared in a
//! snapshot repository. notes starts on version 2, the kernel's own thread swaps
//! it to the pending version without a yes, three calls end probation with a
//! commit, and git acknowledges it. The case's host check then reads the good ref.

use crate::process::components;
use crate::{boot_manifest, hpet, ipc, log, threads};
use alloc::format;
use alloc::string::String;
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

fn run() -> Result<(), String> {
    let (_, Some(pending)) = boot_manifest::labels() else {
        return Err(String::from(
            "manifest: boot with live_pending=notes:<version>",
        ));
    };
    swapped_from("2", &pending)?;
    for _ in 0..3 {
        let reply = call("about")?;
        if !reply.starts_with(&format!("ok notes {pending} ")) || !reply.contains("edited live") {
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
    settle_sync()?;
    let shown = boot_manifest::lines();
    for line in &shown {
        log!("{line}");
    }
    if !shown[0].ends_with(&format!("pending {pending}")) {
        return Err(String::from(
            "manifest: :boot does not show the pending version",
        ));
    }
    log!("manifest: notes started on 2, swapped to {pending} without a yes and committed it");
    Ok(())
}

pub fn check() -> Result<(), &'static str> {
    if crate::build_service::target().is_none() {
        return Err("manifest: boot with build= and build_token=");
    }
    if let Err(why) = crate::live_tests::wait_for_host(0) {
        log!("{why}");
        return Err("manifest: the notes host never came up");
    }
    run().map_err(|why| {
        log!("{why}");
        "manifest: the boot manifest scenario failed"
    })
}
