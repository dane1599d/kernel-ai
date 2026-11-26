//! `test=profile` (P3 task 12's kernel half): resource profiles hold.
//! - `handles mode=mem` gets a 256-page quota (PROC_QUOTA_SET): a commit of
//!   512 pages past it fails with NoMemory, and the process keeps running.
//! - Made `background` (PROC_CLASS_SET), its busy thread shows that class in a
//!   SCHED_STATS window; made `normal` again, it shows that.
//! - A process named `desktop` is never made background, and its quota never
//!   lowered. Without SUPERVISE, the holder itself is refused both calls.
//! - Bug hunt 5 H5-17: a quota lowered under what the holder holds frees no
//!   admission budget while it holds it.

use crate::fpu_tests::wait_exit;
use crate::process::{self, SpawnRequest, profile};
use crate::{ipc, log, sched_control, threads};
use kabi::sched::class;
use kabi::{Error, ExitReason, Limits, cap};

const NAME: &str = "profiled";
const LIMITS: Limits = Limits {
    max_pages: 4096,
    max_burst_ms: 30_000,
    caps: cap::SERVE,
    stack_pages: 0,
};
/// How long the holder spins while the stats window runs.
const SPIN_MS: u64 = 1_500;
const WINDOW_MS: u64 = 500;

fn ask(name: &str, line: &str) -> Result<alloc::string::String, &'static str> {
    let reply = ipc::call(name, line.as_bytes(), 30_000)
        .map_err(|_| "profile: the holder did not answer")?;
    Ok(core::str::from_utf8(&reply).unwrap_or("").into())
}

fn spawn(name: &'static str) -> Result<u64, &'static str> {
    process::spawn(&SpawnRequest {
        name,
        program: "handles",
        args: b"mode=mem",
        limits: LIMITS,
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|_| "profile: cannot start a holder")
}

fn spinner(_: usize) {
    let _ = ask(NAME, &alloc::format!("spin {SPIN_MS}"));
}

/// The class SCHED_STATS shows for `pid`'s threads while it spins.
fn class_seen(pid: u64) -> Result<u64, &'static str> {
    let slot = process::with_table(|t| t.iter().position(|p| p.live() && p.pid == pid))
        .ok_or("profile: the holder is gone")?;
    threads::spawn("spin-call", spinner, 0)?;
    threads::sleep_ms(100);
    let stats = sched_control::stats(slot, WINDOW_MS).map_err(|_| "profile: no stats")?;
    threads::sleep_ms(SPIN_MS);
    stats
        .records()
        .iter()
        .find(|r| r.pid == pid && r.cpu_ns > 0)
        .map(|r| r.class)
        .ok_or("profile: SCHED_STATS shows no thread of the holder")
}

pub fn check() -> Result<(), &'static str> {
    let pid = spawn(NAME)?;
    let desktop = spawn("desktop")?;
    let result = rounds(pid, desktop).and_then(|()| lowered_quota_counts(pid, desktop));
    for name in [NAME, "desktop"] {
        let _ = ask(name, "exit");
    }
    for pid in [pid, desktop] {
        if wait_exit(pid)? != (ExitReason::Exited, 0) {
            return Err("profile: a holder failed");
        }
    }
    result?;
    log!("profile: a quota stopped a commit, a class showed in SCHED_STATS, and the floors held");
    Ok(())
}

fn rounds(pid: u64, desktop: u64) -> Result<(), &'static str> {
    // First, while its quota is roomy: without SUPERVISE it is refused both calls.
    if ask(NAME, "misuse")? != "ok" {
        return Err("profile: the holder's refusals did not hold");
    }
    // Memory.
    if profile::quota_set(pid, 256) != Ok(LIMITS.max_pages) {
        return Err("profile: the quota was not set");
    }
    ask(NAME, "reserve 1024")?;
    let commit = ask(NAME, "commit 512")?;
    if commit != "err commit NoMemory" {
        log!("profile: commit past the quota -> {commit}");
        return Err("profile: a commit past the quota did not fail with NoMemory");
    }
    // CPU.
    profile::class_set(pid, 1).map_err(|_| "profile: cannot make it background")?;
    if class_seen(pid)? != class::BACKGROUND {
        return Err("profile: SCHED_STATS does not show it background");
    }
    profile::class_set(pid, 0).map_err(|_| "profile: cannot make it normal")?;
    if class_seen(pid)? != class::NORMAL {
        return Err("profile: SCHED_STATS does not show it normal again");
    }
    // Floors.
    if profile::class_set(desktop, 1) != Err(Error::Permission)
        || profile::quota_set(desktop, 128) != Err(Error::Permission)
        || profile::quota_set(pid, 8) != Err(Error::Invalid)
    {
        return Err("profile: a floor did not hold");
    }
    Ok(())
}

/// Bug hunt 5 H5-17. The holder takes `TAKEN` pages more (charged, as an
/// `Shm` object's are) under its roomy quota, which is then lowered to the
/// floor with QUOTA_LOWER_ONLY, as a profile's `mem=` does. A raise of
/// desktop's quota into the budget the lower seems to free, counting quotas
/// alone, is refused: the pages are still held. Once they are back, a raise to
/// what the budget has room for, each process counted at its quota or what it
/// holds if that is more, is taken.
fn lowered_quota_counts(pid: u64, desktop: u64) -> Result<(), &'static str> {
    const TAKEN: u64 = 2048;
    let slot = process::slot_of_pid(pid).ok_or("profile: the holder is gone")?;
    // `rounds` left it at 256 pages: roomy again first.
    if profile::quota_set(pid, LIMITS.max_pages) != Ok(256) {
        return Err("profile: the holder's quota was not raised back");
    }
    let taken = process::lock_space(slot)
        .as_mut()
        .is_some_and(|space| space.try_charge(TAKEN, LIMITS.max_pages));
    if !taken {
        return Err("profile: the holder could not take the pages");
    }
    let result = (|| {
        let lowered =
            profile::quota_set_with(pid, profile::MIN_PAGES, kabi::call::QUOTA_LOWER_ONLY);
        if lowered != Ok(LIMITS.max_pages) {
            return Err("profile: the lower-only quota was not set");
        }
        let quotas: u64 = process::with_table(|t| {
            t.iter()
                .filter(|p| p.live())
                .map(|p| p.limits.max_pages)
                .sum()
        });
        let room = process::page_budget().saturating_sub(quotas);
        if room == 0 {
            return Err("profile: the budget has no room for the case");
        }
        match profile::quota_set(desktop, LIMITS.max_pages + room) {
            Err(Error::NoMemory) => Ok(()),
            Ok(_) => Err("profile: a lowered quota freed admission budget its process still held"),
            Err(_) => Err("profile: the raise was refused for another reason"),
        }
    })();
    if let Some(space) = process::lock_space(slot).as_mut() {
        space.refund(TAKEN);
    }
    // Desktop's floor refuses a lower through the call: its quota goes back
    // here, so a step after this one finds the budget as `rounds` left it.
    let restore = || {
        process::with_table(|t| {
            if let Some(p) = t.iter_mut().find(|p| p.live() && p.pid == desktop) {
                p.limits.max_pages = LIMITS.max_pages;
            }
        });
    };
    restore();
    result?;
    let room = process::page_budget().saturating_sub(held_or_quota());
    let raised = profile::quota_set(desktop, LIMITS.max_pages + room);
    restore();
    if raised != Ok(LIMITS.max_pages) {
        return Err("profile: a raise to the room the pages left was refused");
    }
    log!("profile: a quota lowered under what its process holds freed no admission budget");
    Ok(())
}

/// Every live process's quota, or what its space holds if that is more: what
/// the admission rule must count, worked out here from the table and the
/// spaces rather than taken from the kernel's own sum.
fn held_or_quota() -> u64 {
    let mut quotas = [None; process::MAX_PROCESSES];
    process::with_table(|t| {
        for (quota, p) in quotas.iter_mut().zip(t.iter()) {
            *quota = p.live().then_some(p.limits.max_pages);
        }
    });
    let counted = quotas.iter().enumerate().filter_map(|(slot, quota)| {
        let held = process::with_space(slot, |space| space.frames()).unwrap_or(0);
        quota.map(|quota| quota.max(held))
    });
    counted.sum()
}
