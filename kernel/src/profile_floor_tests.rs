//! `test=desk-profile-floor` (P3's exit case): a profile can never starve the
//! system. Each of the floors (P3 plan, "Resource profiles' floors") is tried
//! through the supervisor's `profile` request sent as the shell, the way a
//! profile switch reaches PROC_CLASS_SET and PROC_QUOTA_SET, and the kernel
//! refuses each:
//! - the kernel's reserve: a quota the size of all free memory;
//! - the model holder's pages and KV cache: `llm` made background, or its
//!   quota lowered;
//! - the supervisor, the shell and `desktop`: the same;
//! - free / 2: a raise one page past the admission budget, where one to it
//!   exactly is taken;
//! - system services' CPU share: a service holding TERM made background, where
//!   an app holding none is.

use crate::desk_tests::{desktop_shows, wait};
use crate::process::{self, SpawnRequest};
use crate::{hpet, ipc, log, memory, sched_control};
use alloc::format;
use alloc::string::String;
use kabi::{ExitReason, Limits, cap};

/// The plan line this case adds: `desktop`, a floored service.
pub const PLAN: &str = crate::desk_tests::BOOT_PLAN;
const CALL_MS: u64 = 30_000;
/// An app: no capability of the running system.
const APP: &str = "floor-app";
/// A system service: it holds TERM, as the terminal does.
const SERVICE: &str = "floor-term";
const PAGES: u64 = 1024;

fn spawn(name: &'static str, caps: u64) -> Result<u64, &'static str> {
    process::spawn(&SpawnRequest {
        name,
        program: "handles",
        args: b"mode=mem",
        limits: Limits {
            max_pages: PAGES,
            max_burst_ms: 30_000,
            caps,
            stack_pages: 0,
        },
        spawner: 0,
        supervisor: false,
        uses: &[],
    })
    .map_err(|_| "desk-profile-floor: cannot start a process")
}

fn pid_of(name: &str) -> Result<u64, &'static str> {
    process::named(name)
        .first()
        .map(|&(pid, _)| pid)
        .ok_or("desk-profile-floor: a service is missing")
}

fn quota_of(pid: u64) -> u64 {
    process::with_table(|t| {
        t.iter()
            .find(|p| p.live() && p.pid == pid)
            .map_or(0, |p| p.limits.max_pages)
    })
}

/// The supervisor's answer to `profile <pid> <class> <pages>`, sent as the shell.
fn ask(shell: u64, pid: u64, class: &str, pages: &str) -> Result<String, &'static str> {
    let request = format!("profile {pid} {class} {pages}");
    let deadline = hpet::nanos() + CALL_MS * 1_000_000;
    ipc::call_as("supervisor", request.as_bytes(), shell, deadline, || false)
        .map(|reply| String::from_utf8_lossy(&reply).into_owned())
        .map_err(|_| "desk-profile-floor: the supervisor did not reply")
}

/// Whether `profile <pid> <class> <pages>` was refused with `why`, and left
/// the process as it was.
fn refused(
    shell: u64,
    pid: u64,
    class: &str,
    pages: &str,
    why: &str,
) -> Result<bool, &'static str> {
    let (quota, background) = (quota_of(pid), sched_control::is_background(pid));
    let reply = ask(shell, pid, class, pages)?;
    let same = quota_of(pid) == quota && sched_control::is_background(pid) == background;
    if !reply.starts_with(why) || !same {
        log!("desk-profile-floor: profile {pid} {class} {pages} -> {reply}");
    }
    Ok(reply.starts_with(why) && same)
}

/// Neither made background nor its quota lowered.
fn floored(shell: u64, pid: u64) -> Result<bool, &'static str> {
    let lower = format!("{}", (quota_of(pid) / 2).max(process::profile::MIN_PAGES));
    Ok(refused(shell, pid, "background", "-", "err Permission")?
        && refused(shell, pid, "normal", &lower, "err Permission")?)
}

/// What the admission budget still has room for.
fn room() -> u64 {
    let committed: u64 = process::with_table(|t| {
        t.iter()
            .filter(|p| p.live())
            .map(|p| p.limits.max_pages)
            .sum()
    });
    process::page_budget().saturating_sub(committed)
}

pub fn check() -> Result<(), &'static str> {
    wait("desktop, the shell and llm", || {
        desktop_shows() && !process::named("shell").is_empty() && !process::named("llm").is_empty()
    })?;
    let (app, service) = (
        spawn(APP, cap::SERVE)?,
        spawn(SERVICE, cap::SERVE | cap::TERM)?,
    );
    let result = floors(app, service);
    for pid in [app, service] {
        let _ = process::kill(pid, ExitReason::Killed);
    }
    result?;
    log!("desk-profile-floor: every floor held, whatever the shell asked");
    Ok(())
}

fn floors(app: u64, service: u64) -> Result<(), &'static str> {
    let shell = pid_of("shell")?;
    // The kernel's reserve.
    let free = memory::with(|m| m.free_frames());
    if !refused(shell, app, "normal", &format!("{free}"), "err NoMemory")? {
        return Err("desk-profile-floor: a quota of all free memory was taken");
    }
    log!("desk-profile-floor: the kernel's reserve: a quota of all {free} free pages was refused");
    // The model holder, then the supervisor, the shell and desktop.
    if !floored(shell, pid_of("llm")?)? {
        return Err("desk-profile-floor: llm was slowed or shrunk");
    }
    log!(
        "desk-profile-floor: the model holder: llm was not made background, nor its quota lowered"
    );
    for name in ["supervisor", "shell", "desktop"] {
        if !floored(shell, pid_of(name)?)? {
            log!("desk-profile-floor: {name} was slowed or shrunk");
            return Err("desk-profile-floor: a protected service was slowed or shrunk");
        }
    }
    log!(
        "desk-profile-floor: the supervisor, the shell and desktop were not made background, nor their quotas lowered"
    );
    // free / 2.
    let over = PAGES + room() + 1;
    if !refused(shell, app, "normal", &format!("{over}"), "err NoMemory")? {
        return Err("desk-profile-floor: a raise past the admission budget was taken");
    }
    let to = PAGES + room();
    if !ask(shell, app, "normal", &format!("{to}"))?.starts_with("ok ") || quota_of(app) != to {
        return Err("desk-profile-floor: a raise to the admission budget was refused");
    }
    let _ = ask(shell, app, "normal", &format!("{PAGES}"))?;
    log!(
        "desk-profile-floor: free / 2: a raise one page past the admission budget was refused, and one to it was taken"
    );
    // System services' CPU share.
    if !refused(shell, service, "background", "-", "err Permission")? {
        return Err("desk-profile-floor: a system service was made background");
    }
    if !ask(shell, app, "background", "-")?.starts_with("ok ") || !sched_control::is_background(app)
    {
        return Err("desk-profile-floor: an app was not made background");
    }
    log!(
        "desk-profile-floor: system services' CPU share: a service holding TERM was not made background, and an app was"
    );
    Ok(())
}
