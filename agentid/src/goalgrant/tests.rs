//! The `goals` agent's per-goal ceiling: set only by the person's yes through
//! `kai`, holding only its scope's rows, never a send or an install, ending with
//! the goal.

use super::*;
use crate::mark::Source;
use agentabi::ROWS;
use alloc::vec;

fn person() -> Mark {
    // SAFETY: a test is the input path here.
    Mark::of(unsafe { Source::hardware() })
}

fn scope(rows: &[&str]) -> Vec<String> {
    rows.iter().map(|r| r.to_string()).collect()
}

fn callers(roster: &Roster) -> (Caller, Caller) {
    let kai = roster.caller(Builtin::Kai.id().badge()).unwrap();
    let goals = roster.caller(Builtin::Goals.id().badge()).unwrap();
    (kai, goals)
}

#[test]
fn goals_holds_nothing_until_the_person_approves_a_goal() {
    let roster = Roster::new();
    let (_, goals) = callers(&roster);
    assert_eq!(
        roster.admit(&goals, "fs_list").err(),
        Some(Denied::OutsideGrants)
    );
    assert_eq!(
        roster.admit_for_goal(&goals, 1, "fs_list").err(),
        Some(Denied::OutsideGrants)
    );
}

#[test]
fn an_approved_goal_admits_its_rows_for_that_goal_only() {
    let mut roster = Roster::new();
    let (kai, goals) = callers(&roster);
    let held = roster
        .grant_goal(&kai, person(), 7, &scope(&["fs_list", "fs_trash"]), &ROWS)
        .unwrap();
    assert_eq!(held, ["fs_list", "fs_trash"]);
    let admitted = roster.admit_for_goal(&goals, 7, "fs_trash").unwrap();
    assert_eq!(admitted.goal(), Some(7));
    assert_eq!(admitted.row(), "fs_trash");
    // Another row, another goal, or the plain door: refused.
    assert_eq!(
        roster.admit_for_goal(&goals, 7, "fs_read").err(),
        Some(Denied::OutsideGrants)
    );
    assert_eq!(
        roster.admit_for_goal(&goals, 8, "fs_list").err(),
        Some(Denied::OutsideGrants)
    );
    assert_eq!(
        roster.admit(&goals, "fs_list").err(),
        Some(Denied::OutsideGrants)
    );
    // Only goals is admitted under a goal's ceiling, kai or a package not.
    assert_eq!(
        roster.admit_for_goal(&kai, 7, "fs_list").err(),
        Some(Denied::OutsideGrants)
    );
}

#[test]
fn only_the_persons_yes_through_kai_sets_a_ceiling() {
    let mut roster = Roster::new();
    let (kai, goals) = callers(&roster);
    let rows = scope(&["fs_list"]);
    assert_eq!(
        roster.grant_goal(&goals, person(), 1, &rows, &ROWS),
        Err(GrantRefused::NotKai)
    );
    let typed_by_an_agent = Mark::of(Source::agent(Builtin::Goals.id()));
    assert_eq!(
        roster.grant_goal(&kai, typed_by_an_agent, 1, &rows, &ROWS),
        Err(GrantRefused::NotThePerson)
    );
    assert!(roster.goal_rows(1).is_none());
}

#[test]
fn a_ceiling_never_covers_a_send_or_an_install() {
    let mut roster = Roster::new();
    let (kai, _) = callers(&roster);
    for row in INSTALL_ROWS {
        if agentabi::find(&ROWS, row).is_none() {
            continue;
        }
        let got = roster.grant_goal(&kai, person(), 1, &scope(&["fs_list", row]), &ROWS);
        assert_eq!(
            got,
            Err(GrantRefused::SendsOrInstalls(row.to_string())),
            "{row}"
        );
    }
    // A group that holds one is refused whole, so a scope cannot reach one by `_*`.
    let got = roster.grant_goal(&kai, person(), 1, &scope(&["pkg_*"]), &ROWS);
    assert!(
        matches!(got, Err(GrantRefused::SendsOrInstalls(_))),
        "{got:?}"
    );
    // A change that reaches past the machine is a send.
    let mut send = *agentabi::find(&ROWS, "fs_trash").unwrap();
    send.facts.open_world = true;
    send.facts.read_only = false;
    send.name = "net_send";
    let rows = [send];
    let got = roster.grant_goal(&kai, person(), 1, &scope(&["net_send"]), &rows);
    assert_eq!(
        got,
        Err(GrantRefused::SendsOrInstalls("net_send".to_string()))
    );
    // A read past the machine is no send, so a read-only fetch may be in a scope.
    if agentabi::find(&ROWS, "net_fetch").is_some() {
        assert!(
            roster
                .grant_goal(&kai, person(), 2, &scope(&["net_fetch"]), &ROWS)
                .is_ok()
        );
    }
    assert!(roster.goal_rows(1).is_none());
}

#[test]
fn a_scope_of_no_row_or_too_many_goals_is_refused() {
    let mut roster = Roster::new();
    let (kai, _) = callers(&roster);
    assert_eq!(
        roster.grant_goal(&kai, person(), 1, &scope(&["no_such_row"]), &ROWS),
        Err(GrantRefused::UnknownRow("no_such_row".to_string()))
    );
    assert_eq!(
        roster.grant_goal(&kai, person(), 1, &[], &ROWS),
        Err(GrantRefused::Empty)
    );
    for goal in 0..MAX_GOAL_GRANTS as u64 {
        roster
            .grant_goal(&kai, person(), goal, &scope(&["fs_list"]), &ROWS)
            .unwrap();
    }
    assert_eq!(
        roster.grant_goal(&kai, person(), 99, &scope(&["fs_list"]), &ROWS),
        Err(GrantRefused::Full)
    );
    // A goal already holding one may change it.
    assert!(
        roster
            .grant_goal(&kai, person(), 0, &scope(&["fs_read"]), &ROWS)
            .is_ok()
    );
}

#[test]
fn a_ceiling_ends_with_its_goal_and_a_waiting_call_is_refused_then() {
    let mut roster = Roster::new();
    let (kai, goals) = callers(&roster);
    roster
        .grant_goal(&kai, person(), 3, &scope(&["fs_*"]), &ROWS)
        .unwrap_or_else(|why| panic!("{why}"));
    let waiting = roster.admit_for_goal(&goals, 3, "fs_list").unwrap();
    assert!(roster.revalidate(&waiting).is_ok());
    assert!(roster.revoke_goal(3));
    assert_eq!(
        roster.revalidate(&waiting).err(),
        Some(Denied::OutsideGrants)
    );
    assert!(!roster.revoke_goal(3));
    let _ = vec![0u8];
}
