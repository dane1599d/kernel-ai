//! The boot manifest (M9d): which versions of notes committed in git this boot
//! runs. xtask builds them from their commits, ships them as boot modules and
//! names them on the command line ([`livecore::boot`]). notes starts on the plan's
//! version; once it serves, a kernel thread swaps it to the manifest's version
//! under normal probation. No yes is asked for: the founder decided that a version
//! already committed needs none. Probation, rollback and the git outbox are the
//! usual ones, so git records a rollback and that version is not built again, and
//! a boot that ends during probation tries it again next time.

use crate::process::components;
use crate::{edit_mode, ipc, log, threads};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use livecore::boot::{COMPONENT, Manifest};
use llmproto::text::printable;
use spin::Once;

/// How long notes has to serve before the swap is given up.
const SERVE_WAIT_MS: u64 = 120_000;
const POLL_MS: u64 = 50;

/// How the boot's swap went.
enum Swap {
    /// notes's reply.
    Answered(String),
    /// Why none was sent.
    Skipped(String),
}

static MANIFEST: Once<Manifest> = Once::new();
/// The version notes was serving when the swap thread first saw it.
static STARTED: Once<String> = Once::new();
static SWAPPED: Once<Swap> = Once::new();

/// Reads `live_good=` and `live_pending=` from the kernel command line.
pub fn configure(cmdline: &str) {
    let (manifest, ignored) = Manifest::parse(cmdline);
    for why in ignored {
        log!("boot: {}", printable(&why, 120, 1));
    }
    if !manifest.is_empty() {
        log!("{}", manifest.line());
    }
    MANIFEST.call_once(|| manifest);
}

fn manifest() -> Manifest {
    MANIFEST.get().cloned().unwrap_or_default()
}

/// The labels the manifest names, good and pending.
pub fn labels() -> (Option<String>, Option<String>) {
    let manifest = manifest();
    (manifest.good, manifest.pending)
}

/// notes's plan line: the version the manifest starts it on.
pub fn plan_line() -> String {
    let manifest = manifest();
    let (version, fallback) = manifest.start();
    kabi::plan::notes(version, fallback)
}

/// Starts the thread that swaps notes once it serves, when there is a manifest.
pub fn start() -> Result<(), &'static str> {
    if manifest().is_empty() {
        return Ok(());
    }
    threads::spawn("boot-swap", swap_thread, 0)?;
    Ok(())
}

/// The version notes serves on, once it serves.
fn wait_for_notes() -> Option<String> {
    for _ in 0..SERVE_WAIT_MS / POLL_MS {
        let active = components::ledger(COMPONENT).and_then(|l| l.active().map(String::from));
        if ipc::server_of(COMPONENT).is_some()
            && let Some(active) = active
        {
            return Some(active);
        }
        threads::sleep_ms(POLL_MS);
    }
    None
}

fn swap_thread(_: usize) {
    let manifest = manifest();
    let Some(active) = wait_for_notes() else {
        log!("boot: notes did not serve within {SERVE_WAIT_MS} ms; no swap");
        SWAPPED.call_once(|| Swap::Skipped(String::from("not sent: notes did not serve")));
        return;
    };
    STARTED.call_once(|| active.clone());
    let ledger = components::ledger(COMPONENT).unwrap_or_default();
    let Some(label) = manifest.swap_to(&active, |label| ledger.is_bad(label).is_some()) else {
        SWAPPED.call_once(|| Swap::Skipped(format!("not needed: notes runs {active}")));
        return;
    };
    log!("boot: swapping notes from {active} to {label}, committed in git; no yes needed");
    let reply = edit_mode::swap(COMPONENT, label);
    edit_mode::show(&reply);
    SWAPPED.call_once(|| Swap::Answered(reply));
}

/// What `:boot` prints. The reply from notes keeps the shell's lead, since a
/// program wrote it.
pub fn lines() -> Vec<String> {
    let manifest = manifest();
    if manifest.is_empty() {
        return Vec::from([String::from(
            "boot: no manifest; notes boots version 2 (run --edit names committed versions)",
        )]);
    }
    let (version, fallback) = manifest.start();
    let mut lines = Vec::from([
        manifest.line(),
        format!(
            "  plan: notes version={version}{}",
            fallback.map_or(String::new(), |f| format!(" fallback={f}"))
        ),
    ]);
    let active = components::ledger(COMPONENT).and_then(|l| l.active().map(String::from));
    lines.push(format!(
        "  notes started on {}, runs {}",
        STARTED.get().map_or("?", String::as_str),
        active.as_deref().unwrap_or("nothing")
    ));
    match SWAPPED.get() {
        None => lines.push(String::from("  swap: waiting for notes to serve")),
        Some(Swap::Skipped(why)) => lines.push(format!("  swap: {why}")),
        Some(Swap::Answered(reply)) => {
            lines.push(String::from("  swap, as notes answered it:"));
            // Cut to the screen, so no tail of it wraps onto a row without a lead.
            let (cols, _) = crate::console::size().unwrap_or(edit_mode::FALLBACK_SCREEN);
            let clean = printable(
                reply,
                crate::terminal::PIECE_CHARS.min(cols.saturating_sub(2)),
                1,
            );
            let lead = if clean.starts_with("ok ") { "=" } else { "!" };
            lines.push(format!("{lead} {clean}"));
        }
    }
    lines
}

/// What notes answered to the boot's swap, once it did.
pub fn swap_reply() -> Option<String> {
    match SWAPPED.get() {
        Some(Swap::Answered(reply)) => Some(reply.clone()),
        _ => None,
    }
}
