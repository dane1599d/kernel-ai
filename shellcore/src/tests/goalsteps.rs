//! A goal's steps (G2): `agent-step goal=<id> <call>` runs under that goal's
//! ceiling only, never `goals`' own grants; `end` drops the ceiling; a wait
//! for a step never asked is `err unknown step`.

use super::agentstep::{step, with_agent};
use super::*;
use crate::goalrows::GOALS_ONLY;

const GOALS: u64 = 2;
const SET: &str = r#"{"name":"goal_set","arguments":{"text":"tidy the files in notes/q3","steps":10,"tokens":5000,"wall":"2h"}}"#;
const LIST_Q3: &str = r#"{"name":"fs_list","arguments":{"path":"notes/q3"}}"#;
const RR: &str = r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

/// Goal 1 set on the person's Allow, its add taken and reported.
fn with_goal() -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(SET));
    shell.eval("keep tidying the files in notes/q3");
    let once = host.mint(1, "goal_set(1)");
    assert!(shell.eval(&format!("answer 1 allow once={once}")).ok);
    step(&mut shell, GOALS, "next");
    step(&mut shell, GOALS, "report 1 ok");
    (host, shell)
}

#[test]
fn a_step_inside_the_goals_ceiling_runs_under_its_goal() {
    let (_, mut shell) = with_goal();
    let wire = step(&mut shell, GOALS, &format!("goal=1 {LIST_Q3}"));
    assert!(wire.starts_with("ok run #"), "{wire}");
    let id = shell.log.last_id();
    assert_eq!(shell.log.get(id).unwrap().agent.as_str(), "goals");
}

#[test]
fn a_row_outside_the_ceiling_or_no_goal_is_refused() {
    let (host, mut shell) = with_goal();
    let outside = step(&mut shell, GOALS, &format!("goal=1 {RR}"));
    assert!(outside.starts_with("err refused"), "{outside}");
    // goals holds no grants of its own: a step with no goal is refused.
    let bare = step(&mut shell, GOALS, LIST_Q3);
    assert!(bare.starts_with("err refused"), "{bare}");
    // Another goal's id holds no ceiling.
    let other = step(&mut shell, GOALS, &format!("goal=7 {LIST_Q3}"));
    assert!(other.starts_with("err refused"), "{other}");
    assert_eq!(host.policy.get(), None);
}

#[test]
fn end_drops_the_ceiling_and_later_steps_are_refused() {
    let (_, mut shell) = with_goal();
    assert_eq!(step(&mut shell, GOALS, "goal=1 end"), "ok ended goal 1");
    assert!(shell.roster.goal_rows(1).is_none());
    assert!(shell.goal_book_list().contains("goal 1 ended"));
    let after = step(&mut shell, GOALS, &format!("goal=1 {LIST_Q3}"));
    assert!(after.starts_with("err refused"), "{after}");
}

#[test]
fn only_goals_ends_a_goal() {
    let (_, mut shell, badge) = with_agent();
    assert_eq!(
        step(&mut shell, badge, "goal=1 end"),
        format!("err {GOALS_ONLY}")
    );
}

#[test]
fn a_wait_for_a_step_never_asked_is_unknown() {
    let (_, mut shell) = with_goal();
    assert_eq!(step(&mut shell, GOALS, "wait 99"), "err unknown step #99");
}
