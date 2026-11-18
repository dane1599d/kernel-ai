//! The goals agent's loop over fakes: a scripted planner, a door that asks for
//! `fs_trash`, and steps that record what ran and hand out entries.

use super::*;
use crate::{Budget, Scope, Spec, StepStatus, Tier};
use alloc::collections::VecDeque;
use alloc::string::ToString;
use alloc::vec;

const T0: u64 = 1_000_000;
const HOUR: u64 = 3_600_000;

fn spec() -> Spec {
    Spec {
        text: "tidy the reports folder".to_string(),
        by_ms: Some(T0 + 8 * HOUR),
        scope: Scope {
            rows: vec!["fs_list".to_string(), "fs_trash".to_string()],
            folders: vec!["reports".to_string()],
        },
        budget: Budget {
            steps: 4,
            tokens: 10_000,
            wall_ms: 10 * HOUR,
        },
        checkins: vec![T0 + 2 * HOUR],
        away_ok: true,
    }
}

fn step(row: &str) -> Step<&'static str> {
    Step {
        row: row.to_string(),
        targets: vec!["reports".to_string()],
        tokens_est: 10,
        call: "call",
    }
}

struct Script(VecDeque<Plan<&'static str>>);

impl Planner<&'static str> for Script {
    fn next(&mut self, _goal: &Goal, _now: u64) -> (Plan<&'static str>, u64) {
        (self.0.pop_front().unwrap_or(Plan::Later), 5)
    }
}

struct AsksForTrash;

impl Door<&'static str> for AsksForTrash {
    fn tier(&self, step: &Step<&'static str>) -> Tier {
        match step.row.as_str() {
            "fs_trash" => Tier::Ask,
            _ => Tier::Run,
        }
    }
}

#[derive(Default)]
struct Recorded {
    ran: Vec<(u64, String)>,
    next_entry: u64,
}

impl Steps<&'static str> for Recorded {
    fn run(&mut self, goal: u64, step: &Step<&'static str>) -> StepDone {
        self.ran.push((goal, step.row.clone()));
        self.next_entry += 1;
        StepDone {
            entry: Some(100 + self.next_entry),
            tokens: 7,
            failed: None,
        }
    }
}

/// One turn with the fakes.
fn go(
    runner: &mut Runner<&'static str>,
    goals: &mut Goals,
    now: u64,
    person: Presence,
    planner: &mut Script,
    steps: &mut Recorded,
) -> Vec<Event> {
    let mut parts = Parts {
        planner,
        door: &AsksForTrash,
        steps,
    };
    runner.turn(goals, now, person, &mut parts)
}

fn approved(goals: &mut Goals) -> u64 {
    let id = goals.set(spec(), T0).unwrap();
    goals.approve(id, T0).unwrap();
    id
}

#[test]
fn run_tier_steps_run_unattended_one_a_turn_and_keep_their_entries() {
    let mut goals = Goals::new();
    let id = approved(&mut goals);
    let mut planner = Script(VecDeque::from([
        Plan::Step(step("fs_list")),
        Plan::Step(step("fs_list")),
        Plan::Done("tidy".to_string()),
    ]));
    let (mut steps, mut runner) = (Recorded::default(), Runner::new());
    let mut turn = |now| {
        go(
            &mut runner,
            &mut goals,
            now,
            Presence::Away,
            &mut planner,
            &mut steps,
        )
    };
    let first = turn(T0 + 1);
    assert_eq!(
        first,
        [Event::Ran {
            goal: id,
            step: 1,
            entry: Some(101)
        }]
    );
    let second = turn(T0 + 2);
    assert!(
        matches!(second[0], Event::Ran { step: 2, .. }),
        "{second:?}"
    );
    assert_eq!(turn(T0 + 3), [Event::Done { goal: id }]);
    let goal = goals.get(id).unwrap();
    assert_eq!(goal.state, State::Done);
    assert_eq!(goal.undo_entries(), [102, 101]);
    // Planning's tokens and the steps' are charged.
    assert_eq!(goal.tokens_spent, 3 * 5 + 2 * 7);
    assert_eq!(steps.ran.len(), 2);
}

#[test]
fn a_step_that_asks_waits_runs_on_a_yes_and_is_dropped_on_a_no() {
    for yes in [true, false] {
        let mut goals = Goals::new();
        let id = approved(&mut goals);
        let mut planner = Script(VecDeque::from([Plan::Step(step("fs_trash"))]));
        let (mut steps, mut runner) = (Recorded::default(), Runner::new());
        let events = go(
            &mut runner,
            &mut goals,
            T0 + 1,
            Presence::Here,
            &mut planner,
            &mut steps,
        );
        assert!(
            matches!(&events[..], [Event::Waiting { line, .. }] if line.starts_with("Goal 'tidy the reports folder' is waiting for you:")),
            "{events:?}"
        );
        assert!(
            steps.ran.is_empty(),
            "nothing ran before the person answered"
        );
        // The person answers on the trusted path.
        let goal = goals.get_mut(id).unwrap();
        match yes {
            true => drop(goal.approve(T0 + 2).unwrap()),
            false => goal.deny().unwrap(),
        }
        let events = go(
            &mut runner,
            &mut goals,
            T0 + 3,
            Presence::Here,
            &mut planner,
            &mut steps,
        );
        let goal = goals.get(id).unwrap();
        match yes {
            true => {
                assert_eq!(steps.ran, [(id, "fs_trash".to_string())]);
                assert!(matches!(
                    events[0],
                    Event::Ran {
                        entry: Some(101),
                        ..
                    }
                ));
                assert_eq!(goal.log[0].status, StepStatus::Ran);
            }
            false => {
                assert!(steps.ran.is_empty(), "a no runs nothing");
                assert_eq!(goal.log[0].status, StepStatus::Denied);
            }
        }
    }
}

#[test]
fn a_question_waits_and_a_given_up_goal_fails() {
    let mut goals = Goals::new();
    let id = approved(&mut goals);
    let mut planner = Script(VecDeque::from([
        Plan::Clarify("Which folder holds the reports?".to_string()),
        Plan::Fail("the folder is empty".to_string()),
    ]));
    let (mut steps, mut runner) = (Recorded::default(), Runner::new());
    let mut turn = |goals: &mut Goals, now| {
        go(
            &mut runner,
            goals,
            now,
            Presence::Here,
            &mut planner,
            &mut steps,
        )
    };
    let asked = turn(&mut goals, T0 + 1);
    assert!(
        matches!(&asked[..], [Event::Waiting { line, .. }] if line.ends_with("Which folder holds the reports?")),
        "{asked:?}"
    );
    goals.get_mut(id).unwrap().answered().unwrap();
    assert_eq!(turn(&mut goals, T0 + 2), [Event::Failed { goal: id }]);
    assert_eq!(goals.get(id).unwrap().state, State::Failed);
}

#[test]
fn a_check_in_is_posted_when_it_is_due_and_a_turn_runs_one_step_in_all() {
    let mut goals = Goals::new();
    let a = approved(&mut goals);
    let b = approved(&mut goals);
    let mut planner = Script(VecDeque::from([
        Plan::Step(step("fs_list")),
        Plan::Step(step("fs_list")),
    ]));
    let (mut steps, mut runner) = (Recorded::default(), Runner::new());
    let events = go(
        &mut runner,
        &mut goals,
        T0 + 2 * HOUR,
        Presence::Away,
        &mut planner,
        &mut steps,
    );
    let ran: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            Event::Ran { goal, .. } => Some(*goal),
            _ => None,
        })
        .collect();
    assert_eq!(ran, [a], "one step in all, the first goal's");
    let checked: Vec<u64> = events
        .iter()
        .filter_map(|e| match e {
            Event::CheckIn { goal, .. } => Some(*goal),
            _ => None,
        })
        .collect();
    assert_eq!(checked, [a, b]);
}

#[test]
fn a_goal_the_shell_made_is_adopted_under_its_own_id_once() {
    let mut theirs = Goals::new();
    let id = approved(&mut theirs);
    let goal = theirs.get(id).unwrap().clone();
    let mut ours = Goals::new();
    let mut moved = goal.clone();
    moved.id = 42;
    ours.adopt(moved.clone()).unwrap();
    assert_eq!(ours.get(42).map(|g| g.state), Some(State::Active));
    assert!(ours.adopt(moved).is_err(), "kept already");
    // A goal set later gets an id above every one adopted.
    let next = ours.set(spec(), T0).unwrap();
    assert!(next > 42);
}
