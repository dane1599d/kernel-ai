use super::*;
use alloc::vec;

const HOST: u64 = 7;

/// A state blob in `schema`.
fn blob(schema: u8) -> Vec<u8> {
    vec![0x82, schema, 0x80]
}

fn event(kind: u64, label: &str, schema: u8) -> Event {
    Event {
        kind,
        label: label.into(),
        schema: u64::from(schema),
        blob: blob(schema),
        ..Event::default()
    }
}

fn with_reason(mut event: Event, reason: Reason) -> Event {
    event.reason = reason as u64;
    event
}

/// A ledger whose host (pid 7) runs version 1 in schema 1, and fetched `more`.
fn running(more: &[&str]) -> Ledger {
    let mut ledger = Ledger::new();
    ledger.fetched(HOST, "1");
    for label in more {
        ledger.fetched(HOST, label);
    }
    ledger.apply(HOST, &event(kind::RESTORED, "1", 1)).unwrap();
    ledger
}

fn swapping_to_2() -> Ledger {
    let mut ledger = running(&["2"]);
    ledger.apply(HOST, &event(kind::BEGIN, "2", 1)).unwrap();
    ledger
}

#[test]
fn a_host_restores_only_a_version_it_fetched_in_the_schema_it_claims() {
    let mut ledger = Ledger::new();
    assert_eq!(
        ledger.apply(HOST, &event(kind::RESTORED, "1", 1)),
        Err(Error::Invalid)
    );
    ledger.fetched(HOST, "1");
    let mut wrong = event(kind::RESTORED, "1", 1);
    wrong.blob = blob(2);
    assert_eq!(ledger.apply(HOST, &wrong), Err(Error::Invalid));
    assert_eq!(
        ledger.apply(HOST, &event(kind::RESTORED, "1", 1)),
        Ok(Some("1 started fresh".into()))
    );
    assert_eq!(ledger.active(), Some("1"));
    assert_eq!(
        (ledger.schema, ledger.host_pid, ledger.counters.restores),
        (1, HOST, 1)
    );
    // A later host restores the same version; another version would need a swap.
    ledger.fetched(8, "1");
    ledger.fetched(8, "2");
    assert!(ledger.apply(8, &event(kind::RESTORED, "1", 1)).is_ok());
    assert_eq!(
        ledger.apply(8, &event(kind::RESTORED, "2", 2)),
        Err(Error::Invalid)
    );
    assert_eq!(ledger.counters.rejected, 3);
}

#[test]
fn checkpoints_come_only_from_the_host_in_the_active_schema() {
    let mut ledger = running(&[]);
    let mut checkpoint = event(kind::CHECKPOINT, "", 1);
    checkpoint.blob = vec![0x82, 1, 0x81, 0x01];
    assert_eq!(ledger.apply(HOST, &checkpoint), Ok(None));
    assert_eq!(ledger.checkpoint, checkpoint.blob);
    assert_eq!(ledger.apply(99, &checkpoint), Err(Error::Invalid));
    assert_eq!(
        ledger.apply(HOST, &event(kind::CHECKPOINT, "", 2)),
        Err(Error::Invalid)
    );
    assert_eq!(ledger.counters.checkpoints, 1);
    // Checkpoints are not history.
    assert_eq!(ledger.history.len(), 1);
}

#[test]
fn a_committed_swap_moves_the_active_version_and_its_schema() {
    let mut ledger = running(&["2"]);
    assert_eq!(
        ledger.apply(HOST, &event(kind::BEGIN, "3", 1)),
        Err(Error::Invalid)
    );
    assert_eq!(
        ledger.apply(HOST, &event(kind::BEGIN, "1", 1)),
        Err(Error::Invalid)
    );
    assert_eq!(
        ledger.apply(HOST, &event(kind::BEGIN, "2", 2)),
        Err(Error::Invalid)
    );
    ledger.apply(HOST, &event(kind::BEGIN, "2", 1)).unwrap();
    assert_eq!(ledger.candidate(), Some("2"));
    // While swapping, checkpoints are still the active version's.
    assert!(ledger.apply(HOST, &event(kind::CHECKPOINT, "", 1)).is_ok());
    assert_eq!(
        ledger.apply(HOST, &event(kind::CHECKPOINT, "", 2)),
        Err(Error::Invalid)
    );
    let mut commit = event(kind::COMMIT, "2", 2);
    commit.served = 3;
    assert_eq!(ledger.apply(99, &commit), Err(Error::Invalid));
    assert_eq!(
        ledger.apply(HOST, &commit),
        Ok(Some("1 -> 2 committed after 3 calls".into()))
    );
    assert_eq!(
        (ledger.active(), ledger.schema, ledger.generation),
        (Some("2"), 2, 1)
    );
    assert_eq!(ledger.checkpoint, blob(2));
    assert_eq!(
        (
            ledger.counters.begins,
            ledger.counters.commits,
            ledger.counters.served
        ),
        (1, 1, 3)
    );
}

#[test]
fn a_rollback_blames_the_candidate_unless_it_was_not_its_fault() {
    for (reason, flags, blamed) in [
        (Reason::Trap, 0, true),
        (Reason::Fuel, 0, true),
        (Reason::Trap, flag::POISON, false),
        (Reason::NoMigration, 0, false),
        (Reason::Manual, 0, false),
    ] {
        let mut ledger = swapping_to_2();
        let mut rollback = with_reason(event(kind::ROLLBACK, "2", 1), reason);
        rollback.flags = flags;
        assert!(
            ledger
                .apply(HOST, &rollback)
                .unwrap()
                .unwrap()
                .contains("rolled back")
        );
        assert_eq!(ledger.active(), Some("1"));
        assert_eq!(ledger.is_bad("2").is_some(), blamed, "{reason:?} {flags}");
        assert_eq!(ledger.counters.by_reason[reason as usize], 1);
        assert_eq!(ledger.checkpoint, blob(1));
    }
}

#[test]
fn a_version_that_failed_needs_force_and_a_commit_clears_it() {
    let mut ledger = swapping_to_2();
    ledger
        .apply(
            HOST,
            &with_reason(event(kind::ROLLBACK, "2", 1), Reason::Trap),
        )
        .unwrap();
    assert_eq!(
        ledger.apply(HOST, &event(kind::BEGIN, "2", 1)),
        Err(Error::Permission)
    );
    assert_eq!(ledger.counters.refused_bad, 1);
    assert_eq!(ledger.counters.rejected, 0);
    let mut forced = event(kind::BEGIN, "2", 1);
    forced.flags = flag::FORCE;
    ledger.apply(HOST, &forced).unwrap();
    ledger.apply(HOST, &event(kind::COMMIT, "2", 2)).unwrap();
    assert_eq!(ledger.is_bad("2"), None);
}

#[test]
fn only_a_later_host_reports_that_the_swapping_one_died() {
    let mut ledger = swapping_to_2();
    let died = with_reason(event(kind::ROLLBACK, "2", 1), Reason::HostRestart);
    assert_eq!(ledger.apply(HOST, &died), Err(Error::Invalid));
    let trap = with_reason(event(kind::ROLLBACK, "2", 1), Reason::Trap);
    assert_eq!(ledger.apply(8, &trap), Err(Error::Invalid));
    assert!(ledger.apply(8, &died).is_ok());
    assert_eq!(ledger.active(), Some("1"));
    assert!(ledger.is_bad("2").is_some());
    let mut no_reason = event(kind::ROLLBACK, "2", 1);
    no_reason.reason = 99;
    assert_eq!(swapping_to_2().apply(HOST, &no_reason), Err(Error::Invalid));
}

#[test]
fn a_state_that_cannot_be_restored_disables_until_reset() {
    let mut ledger = running(&[]);
    let fault = Event {
        flags: flag::RESTARTED,
        ..with_reason(event(kind::FAULT, "", 1), Reason::Trap)
    };
    ledger.apply(HOST, &fault).unwrap();
    assert_eq!((ledger.counters.faults, ledger.counters.restarts), (1, 1));
    ledger.fetched(8, "1");
    ledger
        .apply(
            8,
            &with_reason(event(kind::DISABLED, "", 1), Reason::Migrate),
        )
        .unwrap();
    assert!(ledger.disabled);
    assert_eq!(
        ledger.apply(8, &event(kind::RESTORED, "1", 1)),
        Err(Error::Invalid)
    );
    assert_eq!(
        ledger.apply(8, &event(kind::BEGIN, "1", 1)),
        Err(Error::Invalid)
    );
    ledger.apply(8, &event(kind::RESET, "", 0)).unwrap();
    assert!(!ledger.disabled && ledger.checkpoint.is_empty() && ledger.phase == Phase::Empty);
    assert!(ledger.apply(8, &event(kind::RESTORED, "1", 1)).is_ok());
    assert_eq!(
        ledger.apply(8, &event(kind::RESET, "", 0)),
        Err(Error::Invalid)
    );
}

#[test]
fn the_history_keeps_the_last_32_events_in_order() {
    let mut ledger = running(&["2"]);
    for _ in 0..20 {
        ledger.apply(HOST, &event(kind::BEGIN, "2", 1)).unwrap();
        ledger
            .apply(
                HOST,
                &with_reason(event(kind::ROLLBACK, "2", 1), Reason::Manual),
            )
            .unwrap();
    }
    assert_eq!(ledger.history.len(), HISTORY);
    let seqs: Vec<u64> = ledger.history.iter().map(|entry| entry.seq).collect();
    assert_eq!(seqs, (10..42).collect::<Vec<_>>());
    assert_eq!(ledger.history.back().unwrap().kind, kind::ROLLBACK);
}

#[test]
fn a_later_host_can_reset_a_disabled_component() {
    let mut ledger = running(&[]);
    ledger
        .apply(
            HOST,
            &with_reason(event(kind::DISABLED, "", 1), Reason::Migrate),
        )
        .unwrap();
    // The host that disabled it died; the next one loaded the active version.
    assert_eq!(
        ledger.apply(8, &event(kind::RESET, "", 0)),
        Err(Error::Invalid)
    );
    ledger.fetched(8, "1");
    assert!(ledger.apply(8, &event(kind::RESET, "", 0)).is_ok());
    assert!(ledger.apply(8, &event(kind::RESTORED, "1", 1)).is_ok());
    assert_eq!(ledger.host_pid, 8);
}

#[test]
fn huge_counts_from_a_host_saturate_rather_than_overflow() {
    let mut ledger = running(&["2"]);
    for _ in 0..2 {
        ledger.apply(HOST, &event(kind::BEGIN, "2", 1)).unwrap();
        let mut rollback = with_reason(event(kind::ROLLBACK, "2", 1), Reason::Manual);
        rollback.served = u64::MAX;
        rollback.regressions = u64::MAX;
        ledger.apply(HOST, &rollback).unwrap();
    }
    assert_eq!(
        (ledger.counters.served, ledger.counters.regressions),
        (u64::MAX, u64::MAX)
    );
}

#[test]
fn events_about_no_version_carry_no_label() {
    let mut ledger = running(&[]);
    for kind in [kind::FAULT, kind::DISABLED, kind::CHECKPOINT] {
        let labelled = with_reason(event(kind, "\n\x1b[2J", 1), Reason::Trap);
        assert_eq!(ledger.apply(HOST, &labelled), Err(Error::Invalid), "{kind}");
    }
    assert!(
        ledger
            .history
            .iter()
            .all(|entry| entry.label.is_empty() || entry.label == "1")
    );
}

#[test]
fn the_lineage_holds_the_last_active_versions_for_revert() {
    let mut ledger = running(&["2", "3", "4", "5", "6", "7"]);
    assert_eq!(ledger.lineage, ["1"]);
    assert_eq!(ledger.revert_target(1), None);
    for label in ["2", "3", "2", "4", "5", "6", "7"] {
        ledger.apply(HOST, &event(kind::BEGIN, label, 1)).unwrap();
        ledger.apply(HOST, &event(kind::COMMIT, label, 1)).unwrap();
    }
    // At most six, oldest first; a rolled-back swap adds nothing.
    assert_eq!(ledger.lineage, ["3", "2", "4", "5", "6", "7"]);
    ledger.apply(HOST, &event(kind::BEGIN, "1", 1)).unwrap();
    let rollback = with_reason(event(kind::ROLLBACK, "1", 1), Reason::Trap);
    ledger.apply(HOST, &rollback).unwrap();
    assert_eq!(ledger.lineage.len(), 6);
    // The active version is skipped, and each earlier one counted once.
    assert_eq!(ledger.revert_target(1), Some("6"));
    assert_eq!(ledger.revert_target(4), Some("2"));
    assert_eq!(ledger.revert_target(5), Some("3"));
    assert_eq!(ledger.revert_target(6), None);
    assert_eq!(ledger.revert_target(0), None);
    // A host restoring the same version does not add it again.
    ledger.fetched(8, "7");
    ledger.apply(8, &event(kind::RESTORED, "7", 1)).unwrap();
    assert_eq!(ledger.lineage.back().map(String::as_str), Some("7"));
    assert_eq!(ledger.lineage.len(), 6);
}

fn with_detail(mut event: Event, detail: &str) -> Event {
    event.detail = detail.into();
    event
}

#[test]
fn the_latest_trap_with_a_line_names_the_version_that_holds_it() {
    let mut ledger = swapping_to_2();
    let at = "panicked at components/notes/src/lib.rs:58:47";
    // The candidate trapped: the line is in 2's source.
    let rollback = with_detail(with_reason(event(kind::ROLLBACK, "2", 1), Reason::Trap), at);
    ledger.apply(HOST, &rollback).unwrap();
    let located = ledger.last_located.clone().unwrap();
    assert_eq!(
        (
            located.label.as_str(),
            located.at.path.as_str(),
            located.at.line
        ),
        ("2", "components/notes/src/lib.rs", 58)
    );
    // A fault with no line leaves it; one of the active version's moves it to 1.
    let mut fault = with_detail(
        with_reason(event(kind::FAULT, "", 1), Reason::Fuel),
        "out of fuel",
    );
    fault.blob.clear();
    ledger.apply(HOST, &fault).unwrap();
    assert_eq!(ledger.last_located.as_ref().unwrap().label, "2");
    fault = with_detail(
        with_reason(event(kind::FAULT, "", 1), Reason::Trap),
        "panicked at src/lib.rs:7:1",
    );
    fault.blob.clear();
    ledger.apply(HOST, &fault).unwrap();
    assert_eq!(
        ledger
            .last_located
            .as_ref()
            .map(|l| (l.label.as_str(), l.at.line)),
        Some(("1", 7))
    );
}

#[test]
fn a_rollback_the_active_version_caused_locates_the_active_version() {
    let mut ledger = swapping_to_2();
    let mut rollback = with_detail(
        with_reason(event(kind::ROLLBACK, "2", 1), Reason::Trap),
        "panicked at src/lib.rs:3:9",
    );
    rollback.flags = kabi::live::flag::POISON;
    ledger.apply(HOST, &rollback).unwrap();
    assert_eq!(ledger.last_located.unwrap().label, "1");
}
