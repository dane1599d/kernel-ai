//! The goal rows (G2): `goal_set`'s one ask sets the goal's ceiling and
//! queues it for `goals`, which takes it with `agent-step next` and answers
//! with `agent-step report`; pause and resume undo each other; cancel asks and
//! drops the ceiling; no agent sets or changes a goal.

use super::agentstep::{step, with_agent};
use super::*;
use crate::goalrows::{GOALS_ONLY, MAX_QUEUED, PERSON_ONLY};

/// The `goals` agent's badge.
const GOALS: u64 = 2;
const SET: &str = r#"{"name":"goal_set","arguments":{"text":"tidy the files in notes/q3","steps":10,"tokens":5000,"wall":"2h"}}"#;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

/// A shell with goal 1 set on the person's allow (request 1).
fn with_goal() -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(SET));
    let asked = shell.eval("keep tidying the files in notes/q3 for the next two hours");
    assert!(asked.text.contains("goal_set(1)"), "on the card: {asked:?}");
    // `show` gives the whole approval: what it may use and spend.
    let shown = shell.eval("show");
    assert!(
        shown.text.contains("notes/q3") && shown.text.contains("10 steps"),
        "{shown:?}"
    );
    let once = host.mint(1, "goal_set(1)");
    let set = shell.eval(&format!("answer 1 allow once={once}"));
    assert!(set.text.starts_with("Goal 1 set"), "{set:?}");
    (host, shell)
}

fn goals_step(shell: &mut Shell, text: &str) -> String {
    step(shell, GOALS, text)
}

#[test]
fn a_goal_is_set_on_the_persons_allow_with_its_ceiling_and_queued_for_goals() {
    let (_, mut shell) = with_goal();
    let rows = shell
        .roster
        .goal_rows(1)
        .expect("goal 1's ceiling")
        .to_vec();
    assert!(rows.iter().any(|r| r == "fs_list"), "{rows:?}");
    assert!(!rows.iter().any(|r| r.starts_with("goal_")), "{rows:?}");
    let next = goals_step(&mut shell, "next");
    assert!(
        next.starts_with("ok #1 goal-add 1\nkai-goals 1\n"),
        "{next}"
    );
    let file = next.split_once('\n').unwrap().1;
    let goals = goalcore::Goals::parse(file).expect("a kai-goals 1 file");
    let goal = goals.get(1).expect("goal 1");
    assert_eq!(goal.scope.folders, ["notes/q3"]);
    assert!(goal.state.is_live(), "approved: {:?}", goal.state);
    assert_eq!(goals_step(&mut shell, "next"), "ok none");
    assert_eq!(goals_step(&mut shell, "report 1 ok"), "ok reported #1");
    assert!(
        shell.goal_book_list().contains("goal 1 running"),
        "{}",
        shell.goal_book_list()
    );
}

#[test]
fn a_deny_sets_nothing_and_queues_nothing() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(SET));
    shell.eval("keep tidying the files in notes/q3");
    shell.eval("n");
    assert!(shell.roster.goal_rows(1).is_none());
    assert_eq!(goals_step(&mut shell, "next"), "ok none");
}

#[test]
fn a_goal_that_names_nothing_it_does_is_refused_without_an_ask() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(
        r#"{"name":"goal_set","arguments":{"text":"be good","steps":3,"tokens":100,"wall":"1h"}}"#,
    ));
    let reply = shell.eval("be good for an hour");
    assert!(
        !reply.ok && reply.text.contains("say what the goal does"),
        "{reply:?}"
    );
    assert!(shell.pending.is_none());
}

#[test]
fn a_time_not_written_as_minutes_hours_or_days_is_refused() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(
        r#"{"name":"goal_set","arguments":{"text":"tidy notes/q3","steps":3,"tokens":100,"wall":"soon"}}"#,
    ));
    let reply = shell.eval("tidy notes/q3 soon");
    assert!(
        !reply.ok && reply.text.contains("90m, 2h or 1d"),
        "{reply:?}"
    );
}

#[test]
fn pause_and_resume_are_queued_and_undo_each_other() {
    let (host, mut shell) = with_goal();
    goals_step(&mut shell, "next");
    goals_step(&mut shell, "report 1 ok");
    host.calls
        .borrow_mut()
        .push_back(call(r#"{"name":"goal_pause","arguments":{"id":1}}"#));
    let paused = shell.eval("pause goal 1");
    assert!(paused.text.starts_with("Done: goal 1 paused"), "{paused:?}");
    assert_eq!(goals_step(&mut shell, "next"), "ok #2 goal-pause 1");
    let undone = shell.eval_console("undo");
    assert!(undone.ok, "{undone:?}");
    assert_eq!(goals_step(&mut shell, "next"), "ok #3 goal-resume 1");
}

#[test]
fn cancel_asks_and_its_allow_drops_the_ceiling() {
    let (host, mut shell) = with_goal();
    host.calls
        .borrow_mut()
        .push_back(call(r#"{"name":"goal_cancel","arguments":{"id":1}}"#));
    let asked = shell.eval("cancel goal 1");
    assert!(asked.text.contains("goal_cancel(1)"), "{asked:?}");
    assert!(shell.roster.goal_rows(1).is_some(), "not before the yes");
    let id = asked.confirm.unwrap();
    let once = host.mint(id, "goal_cancel(1)");
    let done = shell.eval(&format!("answer {id} allow once={once}"));
    assert!(done.text.starts_with("Goal 1 cancelled"), "{done:?}");
    assert!(shell.roster.goal_rows(1).is_none());
    goals_step(&mut shell, "next");
    assert_eq!(goals_step(&mut shell, "next"), "ok #2 goal-cancel 1");
}

#[test]
fn only_goals_takes_the_queue_and_no_agent_sets_a_goal() {
    let (_, mut shell, badge) = with_agent();
    assert_eq!(step(&mut shell, badge, "next"), format!("err {GOALS_ONLY}"));
    assert_eq!(
        step(&mut shell, badge, "report 1 ok"),
        format!("err {GOALS_ONLY}")
    );
    // goals holds no grant to the goal rows, and the row refuses an agent.
    let set = goals_step(&mut shell, SET);
    assert!(set.starts_with("err refused"), "{set}");
    let _ = PERSON_ONLY;
}

#[test]
fn goals_refusing_a_goal_drops_its_ceiling() {
    let (_, mut shell) = with_goal();
    goals_step(&mut shell, "next");
    goals_step(&mut shell, "report 1 err too many active goals");
    assert!(shell.roster.goal_rows(1).is_none());
    assert!(shell.goal_book_list().contains("goal 1 not started"));
}

#[test]
fn the_queue_holds_at_most_its_limit() {
    let (host, mut shell) = with_goal();
    for n in 0..MAX_QUEUED {
        host.calls.borrow_mut().push_back(call(match n % 2 {
            0 => r#"{"name":"goal_pause","arguments":{"id":1}}"#,
            _ => r#"{"name":"goal_resume","arguments":{"id":1}}"#,
        }));
        let reply = shell.eval("pause or resume goal 1");
        if n + 1 < MAX_QUEUED {
            assert!(reply.ok, "{n}: {reply:?}");
        } else {
            assert!(!reply.ok && reply.text.contains("not taken"), "{reply:?}");
        }
    }
}
