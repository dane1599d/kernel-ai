//! The facts `:why` gives the model about a component's latest rollback, all from
//! the kernel's ledger: which version was dropped and why, which version failed,
//! what the reason means, the detail the host gave, the versions that failed
//! before and the recent events. The detail is text the host wrote, so every value
//! is made printable ASCII on one line.

use crate::Reason;
use crate::ledger::{HistoryEntry, Ledger, Rollback};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::live::kind;

/// Recent events listed.
const RECENT: usize = 8;

/// The version the latest rollback dropped, and its reason's name.
pub fn latest_rollback(ledger: &Ledger) -> Option<(&str, &'static str)> {
    let rollback = ledger.last_rollback.as_ref()?;
    Some((&rollback.label, rollback.reason.name()))
}

/// The facts about `name`'s latest rollback, one per line and at most `max`
/// bytes; None when the ledger holds no rollback.
pub fn facts(name: &str, ledger: &Ledger, max: usize) -> Option<String> {
    let rollback = ledger.last_rollback.as_ref()?;
    let (label, reason) = (plain(&rollback.label), rollback.reason.name());
    let detail = match rollback.detail.is_empty() {
        true => String::from("none recorded"),
        false => plain(&rollback.detail),
    };
    let failed: Vec<String> = ledger
        .bad
        .iter()
        .map(|(label, why)| format!("{} ({})", plain(label), why.name()))
        .collect();
    let recent: Vec<String> = ledger
        .history
        .iter()
        .rev()
        .take(RECENT)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(event)
        .collect();
    let lines = [
        format!("component: {}", plain(name)),
        format!(
            "active version: {}",
            plain(ledger.active().unwrap_or("none"))
        ),
        format!(
            "latest rollback: {label} ({reason}), tried while {} was active",
            plain(&rollback.active)
        ),
        format!("what failed: {}", culprit(rollback)),
        format!("what {reason} means: {}", meaning(rollback.reason)),
        format!("detail from the host: {detail}"),
        format!("versions that failed before: {}", list(&failed)),
        format!("recent events, oldest first: {}", recent.join("; ")),
        format!(
            "rollbacks so far: {}; commits: {}",
            ledger.counters.rollbacks, ledger.counters.commits
        ),
    ];
    let mut out = String::new();
    for line in lines {
        if out.len() + line.len() + 1 > max {
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }
    Some(out)
}

/// Which version the rollback's reason is about. When the active version failed
/// (POISON), the one dropped was not to blame.
fn culprit(rollback: &Rollback) -> String {
    let (label, active) = (plain(&rollback.label), plain(&rollback.active));
    if rollback.not_blamed {
        format!("the active version {active}, not {label}, which is not blamed")
    } else if rollback.reason == Reason::Manual {
        format!("nothing; {label} was rolled back by hand")
    } else {
        format!("{label}, the version on trial")
    }
}

/// One history entry; a fault is always the active version's.
fn event(entry: &HistoryEntry) -> String {
    let what = match entry.kind {
        kind::FAULT => "fault of the active version",
        other => kind::name(other),
    };
    let mut text = format!("#{} {what}", entry.seq);
    for word in [plain(&entry.label), reason_name(entry.reason)] {
        if !word.is_empty() {
            text.push(' ');
            text.push_str(&word);
        }
    }
    text
}

fn reason_name(raw: u64) -> String {
    Reason::from_raw(raw).map_or(String::new(), |reason| String::from(reason.name()))
}

fn list(items: &[String]) -> String {
    match items.is_empty() {
        true => String::from("none"),
        false => items.join(", "),
    }
}

/// `text` as printable ASCII on one line.
fn plain(text: &str) -> String {
    text.chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '?' })
        .collect()
}

/// What a rollback reason says happened, in the words of the rules behind it.
fn meaning(reason: Reason) -> &'static str {
    match reason {
        Reason::Manifest => {
            "its artifact's manifest was missing, malformed or for another component or version"
        }
        Reason::Compile => "its module did not validate or compile",
        Reason::Link => "its module needs an import the host does not offer",
        Reason::Abi => {
            "it broke the component interface: a missing export, a wrong signature, or a result outside the rules"
        }
        Reason::Export => "its state could not be exported, or not in the schema it claimed",
        Reason::NoMigration => {
            "it cannot read the active version's state, and the active version cannot write one it can"
        }
        Reason::Migrate => "moving the state into it failed",
        Reason::SelfTest => "its own self-test failed",
        Reason::Health => "its health check failed",
        Reason::Trap => {
            "it hit a WebAssembly trap, such as an out-of-bounds access or a panic, in one of its calls: a request, its self-test, its health check or a state export"
        }
        Reason::Fuel => "it ran out of fuel: a loop that did not end",
        Reason::Memory => "it asked for more memory than its limit",
        Reason::Host => "a host function refused its arguments",
        Reason::Manual => "it was rolled back by hand",
        Reason::HostRestart => {
            "its host process stopped while it was on probation, so the active version took over"
        }
    }
}

#[cfg(test)]
mod tests;
