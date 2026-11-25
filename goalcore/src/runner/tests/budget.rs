//! Planning at the end of the steps budget (#1044's LOW): a goal that ran its last
//! step is planned once more, so the planner can say it is met, and every other
//! limit still holds it.

use super::*;

/// A goal approved with one step, and a budget of `tokens` and `wall_ms`.
fn one_step(goals: &mut Goals, tokens: u64, wall_ms: u64) -> u64 {
    let budget = Budget {
        steps: 1,
        tokens,
        wall_ms,
    };
    let id = goals.set(Spec { budget, ..spec() }, T0).unwrap();
    goals.approve(id, T0).unwrap();
    id
}

fn pauses(events: Vec<Event>) -> Vec<Event> {
    events
        .into_iter()
        .filter(|event| matches!(event, Event::Paused { .. }))
        .collect()
}

/// Out of steps, the planner still spends: at the token budget the goal pauses for
/// its tokens, and is not planned again. Before the fix the steps budget, checked
/// first, hid the tokens, and planning went on every turn.
#[test]
fn a_goal_out_of_steps_still_pauses_when_planning_uses_up_its_tokens() {
    let mut goals = Goals::new();
    let id = one_step(&mut goals, 20, 10 * HOUR);
    let (mut runner, mut steps) = (Runner::new(), Recorded::default());
    // One step, then a planner that only says "later": 6 tokens a plan, 7 a step.
    let mut first = Script(VecDeque::from([Plan::Step(step("fs_list"))]));
    let ran = turn_with(&mut runner, &mut goals, T0 + 1, &mut first, &mut steps);
    assert!(matches!(ran[..], [Event::Ran { .. }]), "{ran:?}");
    assert_eq!(goals.get(id).unwrap().tokens_spent, 5 + 7);
    let mut planner = Spends {
        calls: 0,
        plan: || Plan::Later,
    };
    let mut paused = Vec::new();
    for at in 2..20 {
        paused.extend(pauses(turn_with(
            &mut runner,
            &mut goals,
            T0 + at,
            &mut planner,
            &mut steps,
        )));
    }
    let tokens = Pause::Budget(Resource::Tokens);
    assert_eq!(
        paused,
        [Event::Paused {
            goal: id,
            pause: tokens
        }]
    );
    let goal = goals.get(id).unwrap();
    assert_eq!(goal.state, State::Paused(tokens));
    // 12, 18, then 24 against 20: at most one plan past the budget.
    assert_eq!((planner.calls, goal.tokens_spent), (2, 24));
}

/// Out of steps and past the wall time, the goal is not planned at all.
#[test]
fn a_goal_out_of_steps_and_out_of_time_is_not_planned() {
    let mut goals = Goals::new();
    let id = one_step(&mut goals, 10_000, HOUR);
    let (mut runner, mut steps) = (Runner::new(), Recorded::default());
    let mut first = Script(VecDeque::from([Plan::Step(step("fs_list"))]));
    turn_with(&mut runner, &mut goals, T0 + 1, &mut first, &mut steps);
    let mut planner = Spends {
        calls: 0,
        plan: || Plan::Done("tidy".to_string()),
    };
    let events = turn_with(&mut runner, &mut goals, T0 + HOUR, &mut planner, &mut steps);
    let wall = Pause::Budget(Resource::Wall);
    assert_eq!(
        pauses(events),
        [Event::Paused {
            goal: id,
            pause: wall
        }]
    );
    assert_eq!(planner.calls, 0);
    assert_eq!(goals.get(id).unwrap().state, State::Paused(wall));
}

/// What the steps budget is left out of planning for: after its last step the
/// planner may say the goal is met; another step pauses it for its steps.
#[test]
fn a_goal_out_of_steps_may_be_said_done_and_takes_no_other_step() {
    for done in [true, false] {
        let mut goals = Goals::new();
        let id = one_step(&mut goals, 10_000, 10 * HOUR);
        let (mut runner, mut steps) = (Runner::new(), Recorded::default());
        let mut planner = Script(VecDeque::from([
            Plan::Step(step("fs_list")),
            match done {
                true => Plan::Done("tidy".to_string()),
                false => Plan::Step(step("fs_list")),
            },
        ]));
        turn_with(&mut runner, &mut goals, T0 + 1, &mut planner, &mut steps);
        let events = turn_with(&mut runner, &mut goals, T0 + 2, &mut planner, &mut steps);
        let out_of_steps = Event::Paused {
            goal: id,
            pause: Pause::Budget(Resource::Steps),
        };
        match done {
            true => assert_eq!(events, [Event::Done { goal: id }]),
            false => assert_eq!(events, [out_of_steps]),
        }
        assert_eq!(steps.ran.len(), 1, "one step, as approved");
    }
}
