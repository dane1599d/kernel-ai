//! The goals agent's loop over fakes: a scripted planner, a door that asks for
//! `fs_trash`, and steps that record what ran and hand out entries.

use super::*;
use crate::{Budget, Pause, Resource, Scope, Spec, StepStatus, Tier};
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
    /// Rows the person says no to on the card.
    deny: Vec<&'static str>,
}

impl Steps<&'static str> for Recorded {
    fn run(&mut self, goal: u64, step: &Step<&'static str>) -> Outcome {
        self.ran.push((goal, step.row.clone()));
        if self.deny.contains(&step.row.as_str()) {
            return Outcome::Denied("the person said no".to_string());
        }
        self.next_entry += 1;
        Outcome::Done(StepDone {
            entry: Some(100 + self.next_entry),
            tokens: 7,
            failed: None,
        })
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

/// A planner that never proposes and charges each time.
struct Spends {
    calls: u32,
    plan: fn() -> Plan<&'static str>,
}

impl Planner<&'static str> for Spends {
    fn next(&mut self, _goal: &Goal, _now: u64) -> (Plan<&'static str>, u64) {
        self.calls += 1;
        ((self.plan)(), 6)
    }
}

fn turn_with(
    runner: &mut Runner<&'static str>,
    goals: &mut Goals,
    now: u64,
    planner: &mut dyn Planner<&'static str>,
    steps: &mut Recorded,
) -> Vec<Event> {
    let mut parts = Parts {
        planner,
        door: &AsksForTrash,
        steps,
    };
    runner.turn(goals, now, Presence::Away, &mut parts)
}

/// The audit's HIGH: planning is held to the budget. A planner that only says
/// "later" (or asks again and again) charges until the budget, then the goal pauses,
/// said, and the planner is not asked again.
#[test]
fn planning_that_spends_past_the_token_budget_pauses_the_goal() {
    let mut goals = Goals::new();
    let id = goals
        .set(
            Spec {
                budget: Budget {
                    tokens: 20,
                    ..spec().budget
                },
                ..spec()
            },
            T0,
        )
        .unwrap();
    goals.approve(id, T0).unwrap();
    let (mut runner, mut steps) = (Runner::new(), Recorded::default());
    let mut planner = Spends {
        calls: 0,
        plan: || Plan::Later,
    };
    let mut paused = Vec::new();
    for at in 1..20 {
        let events = turn_with(&mut runner, &mut goals, T0 + at, &mut planner, &mut steps);
        paused.extend(
            events
                .into_iter()
                .filter(|e| matches!(e, Event::Paused { .. })),
        );
    }
    let goal = goals.get(id).unwrap();
    assert_eq!(
        goal.state,
        State::Paused(Pause::Budget(Resource::Tokens)),
        "{:?}",
        goal.state
    );
    assert_eq!(paused.len(), 1, "paused once, said once: {paused:?}");
    // At most one plan past the budget: 4 plans of 6 tokens is 24 against 20.
    assert_eq!(planner.calls, 4);
    assert_eq!(goal.tokens_spent, 24);
}

#[test]
fn a_full_log_fails_the_goal_said() {
    let mut goals = Goals::new();
    let id = approved(&mut goals);
    // A goal whose log holds as many steps as it may.
    let goal = goals.get_mut(id).unwrap();
    let entry = goal.log.first().cloned();
    let filler = entry.unwrap_or_else(|| crate::StepLog {
        n: 0,
        at_ms: T0,
        proposal: crate::Proposal {
            tier: Tier::Run,
            row: "fs_list".to_string(),
            targets: vec!["reports".to_string()],
            tokens_est: 0,
        },
        status: StepStatus::Denied,
        entry: None,
        tokens: 0,
        note: String::new(),
    });
    for n in 1..=crate::MAX_LOG as u32 {
        goal.log.push(crate::StepLog {
            n,
            ..filler.clone()
        });
    }
    let (mut runner, mut steps) = (Runner::new(), Recorded::default());
    let mut planner = Script(VecDeque::from([Plan::Step(step("fs_list"))]));
    let events = turn_with(&mut runner, &mut goals, T0 + 1, &mut planner, &mut steps);
    assert_eq!(events, [Event::Failed { goal: id }]);
    assert_eq!(goals.get(id).unwrap().state, State::Failed);
    assert!(steps.ran.is_empty());
}

#[test]
fn steps_that_cannot_be_logged_again_and_again_fail_the_goal_said() {
    let mut goals = Goals::new();
    let id = approved(&mut goals);
    let (mut runner, mut steps) = (Runner::new(), Recorded::default());
    let mut planner = Spends {
        calls: 0,
        plan: || Plan::Step(step("Not A Row!")),
    };
    let mut failed = Vec::new();
    for at in 1..10 {
        let events = turn_with(&mut runner, &mut goals, T0 + at, &mut planner, &mut steps);
        failed.extend(
            events
                .into_iter()
                .filter(|e| matches!(e, Event::Failed { .. })),
        );
    }
    assert_eq!(failed, [Event::Failed { goal: id }]);
    assert_eq!(planner.calls, MAX_BAD_STEPS);
    assert_eq!(goals.get(id).unwrap().state, State::Failed);
}

/// The audit's first MEDIUM: a yes, then a pause before the step ran, holds the
/// step until the goal resumes.
#[test]
fn an_approved_step_waits_while_its_goal_is_paused() {
    let mut goals = Goals::new();
    let id = approved(&mut goals);
    let mut planner = Script(VecDeque::from([Plan::Step(step("fs_trash"))]));
    let (mut steps, mut runner) = (Recorded::default(), Runner::new());
    go(
        &mut runner,
        &mut goals,
        T0 + 1,
        Presence::Here,
        &mut planner,
        &mut steps,
    );
    let goal = goals.get_mut(id).unwrap();
    goal.approve(T0 + 2).unwrap();
    goal.pause().unwrap();
    go(
        &mut runner,
        &mut goals,
        T0 + 3,
        Presence::Here,
        &mut planner,
        &mut steps,
    );
    assert!(steps.ran.is_empty(), "nothing runs while it is paused");
    goals.resume(id).unwrap();
    let events = go(
        &mut runner,
        &mut goals,
        T0 + 4,
        Presence::Here,
        &mut planner,
        &mut steps,
    );
    assert_eq!(steps.ran, [(id, "fs_trash".to_string())]);
    assert!(matches!(events[0], Event::Ran { .. }), "{events:?}");
}

/// The audit's second MEDIUM: a goal that waited when the agent restarted is
/// reloaded from its file; a yes then puts a step in flight this runner never saw.
/// It is closed as failed, and the goal is planned again.
#[test]
fn a_step_in_flight_after_a_restart_is_closed_and_the_goal_goes_on() {
    let mut goals = Goals::new();
    let id = approved(&mut goals);
    let mut planner = Script(VecDeque::from([
        Plan::Step(step("fs_trash")),
        Plan::Step(step("fs_list")),
    ]));
    let (mut steps, mut runner) = (Recorded::default(), Runner::new());
    go(
        &mut runner,
        &mut goals,
        T0 + 1,
        Presence::Here,
        &mut planner,
        &mut steps,
    );
    // The agent restarts: the goals come back from their file, the runner is new.
    let mut goals = Goals::parse(&goals.to_text()).unwrap();
    let mut runner = Runner::new();
    goals.get_mut(id).unwrap().approve(T0 + 2).unwrap();
    let events = go(
        &mut runner,
        &mut goals,
        T0 + 3,
        Presence::Here,
        &mut planner,
        &mut steps,
    );
    let goal = goals.get(id).unwrap();
    assert_eq!(goal.log[0].status, StepStatus::Failed);
    assert_eq!(goal.log[0].note, RESTARTED);
    assert!(goal.flight.is_none() || goal.flight == Some(2));
    // The trash step never ran; the goal planned its next one in the same turn.
    assert!(!steps.ran.iter().any(|(_, row)| row == "fs_trash"));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Ran { step: 2, .. })),
        "{events:?}"
    );
}

/// The person's no on the card: the step did not run, and is counted as asked and
/// denied, not as run; the goal reads back from its own file and goes on.
#[test]
fn a_no_on_the_card_is_a_denied_step_not_a_failed_one() {
    let mut goals = Goals::new();
    let id = approved(&mut goals);
    let mut planner = Script(VecDeque::from([
        Plan::Step(step("fs_list")),
        Plan::Step(step("fs_list")),
    ]));
    let mut steps = Recorded {
        deny: vec!["fs_list"],
        ..Recorded::default()
    };
    let mut runner = Runner::new();
    let events = go(
        &mut runner,
        &mut goals,
        T0 + 1,
        Presence::Away,
        &mut planner,
        &mut steps,
    );
    assert_eq!(events, [Event::Denied { goal: id, step: 1 }]);
    let goal = goals.get(id).unwrap();
    assert_eq!(goal.log[0].status, StepStatus::Denied);
    assert_eq!(goal.log[0].note, "the person said no");
    assert_eq!((goal.steps_run, goal.failed, goal.denied), (0, 0, 1));
    assert_eq!(goal.state, State::Active, "it goes on");
    assert!(goal.flight.is_none());
    let text = goals.to_text();
    assert_eq!(Goals::parse(&text).as_ref(), Ok(&goals), "{text}");
}
