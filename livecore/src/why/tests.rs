use super::*;
use crate::ledger::Event;
use alloc::vec;
use kabi::live::flag;

const HOST: u64 = 7;

fn event(kind: u64, label: &str) -> Event {
    Event {
        kind,
        label: label.into(),
        schema: 1,
        blob: vec![0x82, 1, 0x80],
        ..Event::default()
    }
}

fn because(mut event: Event, reason: Reason, detail: &str) -> Event {
    event.reason = reason as u64;
    event.detail = detail.into();
    event
}

/// A ledger whose host runs version 1 and has begun a swap to 2.
fn swapping() -> Ledger {
    let mut ledger = Ledger::new();
    ledger.fetched(HOST, "1");
    ledger.fetched(HOST, "2");
    ledger.apply(HOST, &event(kind::RESTORED, "1")).unwrap();
    ledger.apply(HOST, &event(kind::BEGIN, "2")).unwrap();
    ledger
}

fn rolled_back(reason: Reason, detail: &str, flags: u64) -> Ledger {
    let mut ledger = swapping();
    let mut rollback = because(event(kind::ROLLBACK, "2"), reason, detail);
    rollback.flags = flags;
    ledger.apply(HOST, &rollback).unwrap();
    ledger
}

fn lines(ledger: &Ledger) -> Vec<String> {
    let facts = facts("notes", ledger, 1024).unwrap();
    assert!(facts.ends_with('\n'));
    facts.lines().map(String::from).collect()
}

#[test]
fn the_facts_name_the_latest_rollback_with_its_own_detail() {
    let mut ledger = rolled_back(Reason::Trap, "unreachable\x1b[2J executed", 0);
    // A later fault of the active version leaves its own detail last.
    let fault = because(event(kind::FAULT, ""), Reason::Fuel, "a later fault");
    ledger.apply(HOST, &fault).unwrap();
    assert_eq!(ledger.last_detail, "a later fault");
    assert_eq!(latest_rollback(&ledger), Some(("2", "trap")));
    assert_eq!(
        lines(&ledger),
        [
            "component: notes",
            "active version: 1",
            "latest rollback: 2 (trap), tried while 1 was active",
            "what failed: 2, the version on trial",
            "what trap means: it hit a WebAssembly trap, such as an out-of-bounds access or a panic, in one of its calls: a request, its self-test, its health check or a state export",
            "detail from the host: unreachable?[2J executed",
            "versions that failed before: 2 (trap)",
            "recent events, oldest first: #1 restored 1; #2 begin 2; #3 rollback 2 trap; #4 fault of the active version fuel",
            "rollbacks so far: 1; commits: 0",
        ]
    );
}

#[test]
fn a_rollback_the_active_version_caused_does_not_blame_the_one_dropped() {
    let ledger = rolled_back(Reason::Trap, "unreachable executed in add", flag::POISON);
    let lines = lines(&ledger);
    assert_eq!(
        lines[3],
        "what failed: the active version 1, not 2, which is not blamed"
    );
    assert_eq!(lines[6], "versions that failed before: none");
}

#[test]
fn a_rollback_by_hand_or_without_detail_says_so() {
    let ledger = rolled_back(Reason::Manual, "", 0);
    let lines = lines(&ledger);
    assert_eq!(lines[3], "what failed: nothing; 2 was rolled back by hand");
    assert_eq!(lines[5], "detail from the host: none recorded");
}

#[test]
fn the_latest_rollback_outlasts_the_history() {
    let mut ledger = rolled_back(Reason::Health, "its health check returned 1", 0);
    for _ in 0..crate::ledger::HISTORY + 4 {
        let fault = because(event(kind::FAULT, ""), Reason::Trap, "again");
        ledger.apply(HOST, &fault).unwrap();
    }
    assert!(ledger.history.iter().all(|e| e.kind != kind::ROLLBACK));
    let lines = lines(&ledger);
    assert_eq!(
        lines[2],
        "latest rollback: 2 (health), tried while 1 was active"
    );
    assert_eq!(
        lines[5],
        "detail from the host: its health check returned 1"
    );
}

#[test]
fn no_rollback_means_no_facts() {
    assert_eq!(facts("notes", &swapping(), 1024), None);
    assert_eq!(latest_rollback(&swapping()), None);
    assert_eq!(facts("notes", &Ledger::new(), 1024), None);
}

#[test]
fn the_facts_stop_at_whole_lines_within_the_limit() {
    let ledger = rolled_back(Reason::Trap, "x", 0);
    let facts = facts("notes", &ledger, 100).unwrap();
    assert!(facts.len() <= 100, "{facts:?}");
    assert_eq!(
        facts,
        "component: notes\nactive version: 1\nlatest rollback: 2 (trap), tried while 1 was active\n"
    );
}
