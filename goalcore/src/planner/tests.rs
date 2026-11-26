//! The planner over a scripted llm, turned by the runner.

use super::*;
use crate::runner::{Outcome, Parts, Runner, Steps};
use crate::{
    Budget, Door, Goals, Pause, Presence, Resource, Scope, Spec, State, Step, StepDone, Tier,
};
use alloc::collections::VecDeque;
use alloc::vec;

const T0: u64 = 1_000_000;
const HOUR: u64 = 3_600_000;

/// llm as a list of replies, keeping every request.
struct Scripted {
    replies: VecDeque<Result<String, String>>,
    asked: Vec<String>,
}

impl Llm for Scripted {
    fn call(&mut self, text: &str) -> Result<String, String> {
        self.asked.push(text.to_string());
        let none = Err("no reply".to_string());
        self.replies.pop_front().unwrap_or(none)
    }
}

type Test = JobPlanner<Scripted, &'static str>;

fn scripted(replies: &[Result<&str, &str>]) -> Test {
    let replies = replies
        .iter()
        .map(|r| r.map(String::from).map_err(String::from))
        .collect();
    let llm = Scripted {
        replies,
        asked: Vec::new(),
    };
    JobPlanner::new(llm, |_| Ok("120000 4 0 0\nplan".to_string()), read)
}

/// `ok say` is the goal met and `ok later` a plan next turn; anything else
/// is the planner's failure.
fn read(answer: &str) -> Plan<&'static str> {
    match answer {
        "ok say" => Plan::Done("met".to_string()),
        "ok later" => Plan::Later,
        other => Plan::Fail(other.to_string()),
    }
}

struct Runs;

impl Door<&'static str> for Runs {
    fn tier(&self, _step: &Step<&'static str>) -> Tier {
        Tier::Run
    }
}

struct NoSteps;

impl Steps<&'static str> for NoSteps {
    fn run(&mut self, _goal: u64, _step: &Step<&'static str>) -> Outcome {
        let done = StepDone {
            entry: None,
            tokens: 0,
            failed: None,
        };
        Outcome::Done(done)
    }
}

fn approved(goals: &mut Goals, tokens: u64) -> u64 {
    let budget = Budget {
        steps: 4,
        tokens,
        wall_ms: 10 * HOUR,
    };
    let scope = Scope {
        rows: vec!["fs_list".to_string()],
        folders: vec!["reports".to_string()],
    };
    let spec = Spec {
        text: "tidy the reports folder".to_string(),
        by_ms: Some(T0 + 8 * HOUR),
        scope,
        budget,
        checkins: vec![T0 + 2 * HOUR],
        away_ok: true,
    };
    let id = goals.set(spec, T0).unwrap();
    goals.approve(id, T0).unwrap();
    id
}

/// `n` turns of the runner: the goal's state after each.
fn turns(goals: &mut Goals, id: u64, planner: &mut Test, n: u64) -> Vec<State> {
    let mut runner = Runner::new();
    let mut states = Vec::new();
    for at in 1..=n {
        let mut parts = Parts {
            planner: &mut *planner,
            door: &Runs,
            steps: &mut NoSteps,
        };
        runner.turn(goals, T0 + at, Presence::Here, &mut parts);
        states.push(goals.get(id).unwrap().state);
    }
    states
}

#[test]
fn a_take_waits_plans_afresh_or_answers() {
    assert_eq!(taken(Err("llm: TimedOut")), Taken::Wait);
    assert_eq!(taken(Ok("pending")), Taken::Wait);
    assert_eq!(taken(Ok("busy Kai is writing an app")), Taken::Wait);
    assert_eq!(taken(Ok("ended unknown ticket")), Taken::Replan);
    for text in ["ok ok say", "err malformed act request", "ended timeout"] {
        assert_eq!(taken(Ok(text)), Taken::Answer { text, used: 0 });
    }
    let used = taken(Ok("used 3000 ok ok say"));
    assert_eq!(
        used,
        Taken::Answer {
            text: "ok ok say",
            used: 3000
        }
    );
}

/// bug-hunt-4 H4-13: llm that cannot answer now fails no goal. Before, a take
/// that timed out, `busy` and a ticket llm forgot on restart each failed it.
#[test]
fn llm_that_cannot_answer_now_fails_no_goal() {
    let mut goals = Goals::new();
    let id = approved(&mut goals, 10_000);
    let mut planner = scripted(&[
        Ok("ok 7"),
        Err("llm: TimedOut"),
        Ok("busy Kai is writing an app"),
        Ok("ended unknown ticket"),
        Ok("ok 8"),
        Ok("ok say"),
    ]);
    let states = turns(&mut goals, id, &mut planner, 6);
    assert_eq!(states[..5], [State::Active; 5]);
    assert_eq!(states[5], State::Done);
    let asked = &planner.llm.asked;
    assert!(asked[4].starts_with("agent-start act "), "{asked:?}");
    assert_eq!(asked[3], "agent-take 7");
    assert_eq!(asked[5], "agent-take 8");
}

/// A job's own failure still fails the goal.
#[test]
fn the_jobs_own_failure_still_fails_the_goal() {
    let mut goals = Goals::new();
    let id = approved(&mut goals, 10_000);
    let mut planner = scripted(&[Ok("ok 7"), Ok("ended timeout")]);
    let states = turns(&mut goals, id, &mut planner, 2);
    assert_eq!(states, [State::Active, State::Failed]);
}

/// bug-hunt-4 H4-14: a goal cancelled with its plan job in flight stops the
/// job in llm, so it neither runs nor holds one of goals' places there.
#[test]
fn a_cancelled_goals_job_is_stopped_in_llm() {
    let mut goals = Goals::new();
    let id = approved(&mut goals, 10_000);
    let mut planner = scripted(&[Ok("ok 7"), Ok("pending"), Ok("ok")]);
    turns(&mut goals, id, &mut planner, 2);
    planner.forget(id);
    planner.forget(id);
    assert_eq!(planner.llm.asked[2..], ["agent-stop 7"]);
}

/// A goal that ended any other way has its job stopped too, once.
#[test]
fn the_jobs_of_goals_that_ended_are_stopped() {
    let mut goals = Goals::new();
    let id = approved(&mut goals, 10_000);
    let mut planner = scripted(&[Ok("ok 7"), Ok("ok")]);
    turns(&mut goals, id, &mut planner, 1);
    planner.drop_ended(&goals);
    assert_eq!(planner.llm.asked.len(), 1, "a live goal's job is kept");
    goals.get_mut(id).unwrap().cancel(T0 + 2).unwrap();
    planner.drop_ended(&goals);
    planner.drop_ended(&goals);
    assert_eq!(planner.llm.asked[1..], ["agent-stop 7"]);
}

/// A stop llm did not answer `ok` (busy writing the person's app, or no
/// reply) is asked again each turn until it is, and then never again.
#[test]
fn a_stop_llm_did_not_take_is_asked_again() {
    let mut goals = Goals::new();
    let id = approved(&mut goals, 10_000);
    let mut planner = scripted(&[
        Ok("ok 7"),
        Ok("busy Kai is writing an app"),
        Err("llm: TimedOut"),
        Ok("ok"),
    ]);
    turns(&mut goals, id, &mut planner, 1);
    goals.get_mut(id).unwrap().cancel(T0 + 2).unwrap();
    planner.forget(id);
    planner.drop_ended(&goals);
    planner.drop_ended(&goals);
    planner.drop_ended(&goals);
    assert_eq!(planner.llm.asked[1..], ["agent-stop 7"; 3]);
}

/// bug-hunt-4 H4-15: what each plan used is charged to the goal, so the token
/// budget on the person's card pauses it. Before, every plan was charged 0.
#[test]
fn what_planning_used_pauses_the_goal_at_its_tokens() {
    let mut goals = Goals::new();
    let id = approved(&mut goals, 5_000);
    let later = Ok("used 3000 ok later");
    let mut planner = scripted(&[Ok("ok 7"), later, Ok("ok 8"), later]);
    let states = turns(&mut goals, id, &mut planner, 4);
    let tokens = State::Paused(Pause::Budget(Resource::Tokens));
    assert_eq!(
        states,
        [State::Active, State::Active, State::Active, tokens]
    );
    assert_eq!(goals.get(id).unwrap().tokens_spent, 6_000);
}

/// H6-28: the planner's line lists only a step that ran as done; a failed, a
/// refused and a held one are told with why, and no "next" is made up.
#[test]
fn the_plan_line_calls_only_a_step_that_ran_done() {
    use crate::{Proposal, StepLog};
    let mut goals = Goals::new();
    let id = approved(&mut goals, 100_000);
    let goal = goals.get_mut(id).unwrap();
    let nothing = plan_line(goal);
    assert_eq!(nothing, "goal: tidy the reports folder\ndone: nothing yet");
    let log = |n: u32, row: &str, status: StepStatus, note: &str| StepLog {
        n,
        at_ms: T0,
        proposal: Proposal {
            row: row.to_string(),
            targets: Vec::new(),
            tier: Tier::Run,
            tokens_est: 0,
        },
        status,
        entry: None,
        tokens: 0,
        note: note.to_string(),
    };
    goal.log.push(log(1, "fs_read", StepStatus::Ran, ""));
    goal.log.push(log(
        2,
        "fs_write",
        StepStatus::Failed,
        "no such folder notes",
    ));
    goal.log.push(log(3, "fs_trash", StepStatus::Denied, ""));
    goal.log.push(log(
        4,
        "fs_move",
        StepStatus::Held,
        "the person said no\nbefore",
    ));
    let line = plan_line(goal);
    assert_eq!(
        line,
        "goal: tidy the reports folder\ndone: fs_read\n\
         not done: fs_move held back: the person said no?before; \
         fs_trash refused by the person; fs_write failed: no such folder notes"
    );
    // What the goal said it plans next is passed on, and only then.
    goal.plan(3, "move the files");
    assert!(plan_line(goal).ends_with("\nnext: move the files"));
}
