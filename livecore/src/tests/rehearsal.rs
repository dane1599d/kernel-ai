//! `ctl try`: a rehearsed swap reports each step, changes no state, and names the
//! step that failed.

use super::*;
use alloc::format;

/// What the ledger records, which a try must leave as it was.
fn record(kernel: &Kernel) -> String {
    let ledger = ledger(kernel);
    format!(
        "{:?} {:?} {:?} {} {} {:?}",
        ledger.active(),
        ledger.counters,
        ledger.history,
        ledger.generation,
        ledger.schema,
        ledger.checkpoint
    )
}

#[test]
fn a_try_reports_every_step_and_changes_nothing() {
    let kernel = kernel(&[V1, V2]);
    let mut host = host(&kernel);
    call(&mut host, "add");
    call(&mut host, "add");
    let before = record(&kernel);
    assert_eq!(
        call(&mut host, "ctl try 2"),
        "ok try 2: import ok, self-test ok, health ok, export ok, 2 replayed, reverse none"
    );
    assert_eq!(record(&kernel), before);
    assert_eq!(ledger(&kernel).counters.begins, 0);
    assert_eq!(kernel.component.borrow().fetches("2"), 1);
    // The live version saw neither the migration nor the replay.
    assert_eq!(call(&mut host, "get"), "ok 2 schema 1");
}

#[test]
fn a_candidate_that_writes_the_old_schema_is_read_back() {
    let back = Spec {
        writes: &[1],
        ..Spec { label: "3", ..V2 }
    };
    let kernel = kernel(&[V1, back]);
    let mut host = host(&kernel);
    call(&mut host, "add");
    assert_eq!(
        call(&mut host, "ctl try 3"),
        "ok try 3: import ok, self-test ok, health ok, export ok, 1 replayed, reverse ok"
    );
}

#[test]
fn a_failed_try_names_the_step_and_changes_nothing() {
    let older_trap = Spec {
        writes: &[1],
        ..broken("3", Bug::ExportOlderTrap)
    };
    for (spec, expected) in [
        (broken("3", Bug::Link), "link: unknown import kai.teleport"),
        (broken("3", Bug::ImportTrap), "migrate: "),
        (broken("3", Bug::SelfTest), "self-test: "),
        (broken("3", Bug::Health), "health: "),
        (broken("3", Bug::Export), "export: "),
        (
            broken("3", Bug::Regress),
            "1 of 1 replayed requests answered err where 1 answered ok",
        ),
        (
            broken("3", Bug::TrapAt(2)),
            "replaying request 1 of 1: trap: ",
        ),
        (older_trap, "writing schema 1 for 1: trap: "),
    ] {
        let kernel = kernel(&[V1, spec]);
        let mut host = host(&kernel);
        call(&mut host, "add");
        let before = record(&kernel);
        let reply = call(&mut host, "ctl try 3");
        assert!(
            reply.starts_with(&format!("err try 3: {expected}")),
            "{reply}"
        );
        assert_eq!(record(&kernel), before, "{reply}");
        assert_eq!(call(&mut host, "get"), "ok 1 schema 1");
    }
}

#[test]
fn a_try_needs_a_serving_host_and_a_stored_version() {
    let kernel = kernel(&[V1, V2]);
    let mut host = host(&kernel);
    assert_eq!(
        call(&mut host, "ctl try 1"),
        "err try 1: 1 is already active"
    );
    assert_eq!(call(&mut host, "ctl try 9"), "err try 9: no version 9");
    assert_eq!(
        call(&mut host, "ctl try ../x"),
        "err bad version label \"../x\""
    );
    let spoofed = host.handle(4242, b"ctl try 2").unwrap();
    assert_eq!(spoofed, b"err control requests come only from the kernel");
    assert_eq!(call(&mut host, "ctl swap 2"), "ok probation 2 (3 calls)");
    assert_eq!(
        call(&mut host, "ctl try 2"),
        "err busy: a version is on probation"
    );
}

#[test]
fn the_replay_runs_the_last_requests_served_without_a_fault() {
    let kernel = kernel(&[V1, V2]);
    let mut host = host(&kernel);
    for _ in 0..10 {
        call(&mut host, "add");
    }
    call(&mut host, "poison");
    let reply = call(&mut host, "ctl try 2");
    assert!(reply.contains(", 8 replayed,"), "{reply}");
}

#[test]
fn a_request_the_old_copy_fails_ends_the_replay_without_failing_the_try() {
    let trapping = Spec {
        bug: Bug::TrapAt(4),
        ..V1
    };
    let kernel = kernel(&[trapping, V2]);
    let mut host = host(&kernel);
    for _ in 0..3 {
        call(&mut host, "add");
    }
    let reply = call(&mut host, "ctl try 2");
    assert!(
        reply.contains(", 0 of 3 replayed (1 failed the next),"),
        "{reply}"
    );
}
