//! Terminal commands for live components: what the kernel's ledger says, and
//! control requests sent to a component's host as the kernel (caller 0). Replies
//! from a host are printed with the shell's `= ` and `! ` leads, since a program
//! wrote them.

use crate::process::components;
use crate::{ipc, log};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::live::{Reason, kind};
use livecore::ledger::Ledger;
use livecore::store::Origin;
use llmproto::text::printable;

const TIMEOUT_MS: u64 = 60_000;
const MAX_LINES: usize = 64;

pub const HELP: &[(&str, &str)] = &[
    (":comp [name]", "live components: versions, ledger, history"),
    (
        ":swap <name> <version> [force]",
        "swap a component to another version",
    ),
    (":rollback <name>", "drop the version on probation"),
    (
        ":source <name> [file]",
        "the active version's source; :edit changes it",
    ),
    (":state <name>", "the state the component last saved"),
    (":call <service> <request>", "send a request to a service"),
];

/// Runs `command` if it is one of these; false otherwise.
pub fn run(command: &str, rest: &str) -> bool {
    let words: Vec<&str> = rest.split_whitespace().collect();
    match (command, words.as_slice()) {
        ("comp", []) => list(),
        ("comp", [name]) => show(name),
        ("swap", [name, label]) => control(name, &format!("swap {label}")),
        ("swap", [name, label, "force"]) => control(name, &format!("swap {label} force")),
        ("rollback", [name]) => control(name, "rollback"),
        ("source", [name]) => control(name, "source"),
        ("source", [name, file]) => control(name, &format!("source {file}")),
        ("state", [name]) => match components::ledger(name) {
            Some(ledger) => state(name, &ledger).iter().for_each(|line| log!("{line}")),
            None => log!("no live component {name}"),
        },
        ("call", [service, ..]) => {
            let request = rest.trim_start().strip_prefix(service).unwrap_or("").trim();
            call(service, request);
        }
        ("comp" | "swap" | "rollback" | "source" | "state" | "call", _) => {
            let usage = HELP
                .iter()
                .find(|(usage, _)| usage[1..].starts_with(command));
            log!("usage: {}", usage.map_or("", |(usage, _)| usage));
        }
        _ => return false,
    }
    true
}

/// What `:state <name>` prints for `ledger`, fit to the screen: every line on one
/// row, and none scrolled away before the prompt.
pub fn state(name: &str, ledger: &Ledger) -> Vec<String> {
    let (cols, rows) = crate::console::size().unwrap_or(crate::edit_mode::FALLBACK_SCREEN);
    let width = crate::terminal::PIECE_CHARS.min(cols.saturating_sub(2));
    livecore::state::lines(name, ledger, width, MAX_LINES.min(rows.saturating_sub(2)))
}

fn control(name: &str, verb: &str) {
    call(name, &format!("{}{verb}", kabi::live::CONTROL_PREFIX));
}

fn call(service: &str, request: &str) {
    match ipc::call(service, request.as_bytes(), TIMEOUT_MS) {
        Ok(reply) => {
            let text = String::from_utf8_lossy(&reply);
            let clean = printable(&text, kabi::MAX_MESSAGE, MAX_LINES);
            let (lead, body) = match clean.split_once(' ').unwrap_or((clean.as_str(), "")) {
                ("ok", body) => ("=", body),
                ("err", body) => ("!", body),
                _ => ("!", clean.as_str()),
            };
            for line in body.lines() {
                log!("{lead} {line}");
            }
        }
        Err(error) => log!("{service}: no answer ({error:?})"),
    }
}

fn list() {
    let all = components::versions();
    if all.is_empty() {
        log!("no live components");
    }
    for (name, labels) in all {
        let active = components::ledger(&name)
            .and_then(|ledger| ledger.active().map(String::from))
            .unwrap_or_else(|| String::from("none"));
        log!("  {name}: active {active}; versions {}", labels.join(" "));
    }
}

fn show(name: &str) {
    let Some(ledger) = components::ledger(name) else {
        log!("no live component {name}");
        return;
    };
    let candidate = ledger
        .candidate()
        .map_or(String::new(), |c| format!(", swapping to {c}"));
    log!(
        "{name}: active {} schema {}{candidate}; generation {}{}",
        ledger.active().unwrap_or("none"),
        ledger.schema,
        ledger.generation,
        if ledger.disabled { "; DISABLED" } else { "" }
    );
    let fetches: Vec<String> = components::versions()
        .into_iter()
        .filter(|(n, _)| n == name)
        .flat_map(|(_, labels)| labels)
        .map(|label| format!("{label} ({})", components::fetches(name, &label)))
        .collect();
    log!("  versions (fetches): {}", fetches.join(" "));
    if !ledger.bad.is_empty() {
        let bad: Vec<String> = ledger
            .bad
            .iter()
            .map(|(label, why)| format!("{label} ({})", why.name()))
            .collect();
        log!("  failed: {}", bad.join(", "));
    }
    let c = &ledger.counters;
    log!(
        "  restores {} checkpoints {} begins {} commits {} rollbacks {} faults {} refused {}",
        c.restores,
        c.checkpoints,
        c.begins,
        c.commits,
        c.rollbacks,
        c.faults,
        c.refused_bad
    );
    for entry in ledger.history.iter().rev().take(8) {
        let reason = Reason::from_raw(entry.reason).map_or("", Reason::name);
        log!(
            "  #{} {} {} {reason}",
            entry.seq,
            kind::name(entry.kind),
            printable(&entry.label, kabi::live::MAX_LABEL, 1)
        );
    }
    if !ledger.last_detail.is_empty() {
        log!("  last detail: {}", printable(&ledger.last_detail, 120, 1));
    }
    show_uploads(name);
}

/// Uploads, where each came from, the next label, the lineage and git sync.
fn show_uploads(name: &str) {
    let Some(component) = components::component(name) else {
        return;
    };
    for version in component.versions() {
        if let Origin::Upload { base, commit, .. } = &version.origin {
            log!(
                "  upload {} from {base}, commit {}, {} KiB",
                version.label,
                commit.get(..12).unwrap_or(""),
                version.bytes.as_slice().len() / 1024
            );
        }
    }
    let lineage: Vec<&str> = component
        .ledger
        .lineage
        .iter()
        .map(String::as_str)
        .collect();
    let uploads = &component.uploads;
    log!(
        "  next upload {}; uploads accepted {} refused {} evicted {}; ran {}",
        component.next_label(),
        uploads.accepted,
        uploads.rejected,
        uploads.evicted,
        lineage.join(" -> ")
    );
    let outbox = &uploads.outbox;
    if outbox.enabled() {
        let last = outbox.synced().last().map_or(String::from("none"), |job| {
            let sha = job.sha.as_deref().unwrap_or("");
            format!("{} at {}", job.label, sha.get(..12).unwrap_or("untracked"))
        });
        log!(
            "  git sync: {} waiting, {} dropped; last {last}",
            outbox.pending(),
            outbox.dropped
        );
    }
}
