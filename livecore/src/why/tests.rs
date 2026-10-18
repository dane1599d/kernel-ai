use super::*;
use crate::ledger::{HistoryEntry, Phase};

fn entry(seq: u64, kind: u64, reason: Option<Reason>, label: &str, detail: &str) -> HistoryEntry {
    HistoryEntry {
        seq,
        kind,
        reason: reason.map_or(0, |reason| reason as u64),
        label: label.into(),
        detail: detail.into(),
    }
}

/// notes on 2 after 3-trap trapped on probation, then 2 failed on a request.
fn ledger() -> Ledger {
    let mut ledger = Ledger::new();
    ledger.phase = Phase::Steady { active: "2".into() };
    ledger.bad.push(("3-trap".into(), Reason::Trap));
    ledger.counters.rollbacks = 1;
    ledger.counters.commits = 1;
    ledger.history.extend([
        entry(1, kind::RESTORED, None, "2", ""),
        entry(2, kind::CHECKPOINT, None, "", ""),
        entry(3, kind::BEGIN, None, "3-trap", ""),
        entry(
            4,
            kind::ROLLBACK,
            Some(Reason::Trap),
            "3-trap",
            "unreachable\x1b[2J executed",
        ),
        entry(5, kind::FAULT, Some(Reason::Fuel), "2", "a later fault"),
    ]);
    ledger.last_detail = "a later fault".into();
    ledger
}

#[test]
fn the_facts_name_the_latest_rollback_with_its_own_detail() {
    let ledger = ledger();
    assert_eq!(latest_rollback(&ledger), Some(("3-trap", "trap")));
    let facts = facts("notes", &ledger, 1024).unwrap();
    let lines: Vec<&str> = facts.lines().collect();
    assert_eq!(
        lines,
        [
            "component: notes",
            "active version: 2",
            "latest rollback: 3-trap (trap)",
            "what trap means: it hit a WebAssembly trap, such as an out-of-bounds access or a panic, while serving a request",
            "detail from the host: unreachable?[2J executed",
            "versions that failed before: 3-trap (trap)",
            "recent events, oldest first: #1 restored 2; #3 begin 3-trap; #4 rollback 3-trap trap; #5 fault 2 fuel",
            "rollbacks so far: 1; commits: 1",
        ]
    );
    assert!(facts.ends_with('\n'));
}

#[test]
fn a_rollback_without_detail_or_earlier_failures_says_so() {
    let mut ledger = Ledger::new();
    ledger.phase = Phase::Steady { active: "2".into() };
    ledger.history.push_back(entry(
        9,
        kind::ROLLBACK,
        Some(Reason::HostRestart),
        "3-spin",
        "",
    ));
    let facts = facts("notes", &ledger, 1024).unwrap();
    assert!(facts.contains("latest rollback: 3-spin (host-restart)\n"));
    assert!(facts.contains("its host process stopped while it was on probation"));
    assert!(facts.contains("detail from the host: none recorded\n"));
    assert!(facts.contains("versions that failed before: none\n"));
}

#[test]
fn no_rollback_means_no_facts() {
    let mut ledger = ledger();
    ledger.history.retain(|entry| entry.kind != kind::ROLLBACK);
    assert_eq!(facts("notes", &ledger, 1024), None);
    assert_eq!(latest_rollback(&ledger), None);
    assert_eq!(facts("notes", &Ledger::new(), 1024), None);
}

#[test]
fn the_facts_stop_at_whole_lines_within_the_limit() {
    let facts = facts("notes", &ledger(), 80).unwrap();
    assert!(facts.len() <= 80, "{facts:?}");
    assert_eq!(
        facts,
        "component: notes\nactive version: 2\nlatest rollback: 3-trap (trap)\n"
    );
}
