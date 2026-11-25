//! A step the shell was too busy to take (`Outcome::Busy`, #1362's review): it
//! never reached the door, so it is sent again after a wait that grows, never
//! past the goal's deadline or wall time, and a cancel meanwhile logs it as
//! held, not as run and failed.

use super::*;

/// Steps the shell is too busy to take for the first `busy` sends; then they
/// run. Each send is recorded with its time.
struct Busy {
    sends: usize,
    busy: usize,
}

impl Steps<&'static str> for Busy {
    fn run(&mut self, _goal: u64, _step: &Step<&'static str>) -> Outcome {
        self.sends += 1;
        if self.sends <= self.busy {
            return Outcome::Busy;
        }
        Outcome::Done(StepDone {
            entry: Some(300),
            tokens: 0,
            failed: None,
        })
    }
}

fn turn(
    runner: &mut Runner<&'static str>,
    goals: &mut Goals,
    now: u64,
    busy: &mut Busy,
) -> Vec<Event> {
    let mut planner = Script(VecDeque::from([Plan::Step(step("fs_list"))]));
    let mut parts = Parts {
        planner: &mut planner,
        door: &AsksForTrash,
        steps: busy,
    };
    runner.turn(goals, now, Presence::Here, &mut parts)
}

#[test]
fn a_busy_step_is_sent_again_after_a_wait_that_doubles() {
    let mut goals = Goals::new();
    let id = approved(&mut goals);
    let mut busy = Busy { sends: 0, busy: 2 };
    let mut runner = Runner::new();
    turn(&mut runner, &mut goals, T0 + 1, &mut busy);
    assert_eq!(busy.sends, 1);
    assert_eq!(goals.get(id).unwrap().flight, Some(1));
    // Not sent again each turn: first after BUSY_FIRST_MS.
    for now in [T0 + 2, T0 + 1_000, T0 + BUSY_FIRST_MS] {
        turn(&mut runner, &mut goals, now, &mut busy);
    }
    assert_eq!(busy.sends, 1, "held for the first wait");
    turn(&mut runner, &mut goals, T0 + 1 + BUSY_FIRST_MS, &mut busy);
    assert_eq!(busy.sends, 2);
    // Busy again: the next wait is twice as long.
    let second = T0 + 1 + BUSY_FIRST_MS;
    turn(&mut runner, &mut goals, second + BUSY_FIRST_MS, &mut busy);
    assert_eq!(busy.sends, 2, "the wait doubled");
    let ran = turn(
        &mut runner,
        &mut goals,
        second + 2 * BUSY_FIRST_MS,
        &mut busy,
    );
    assert_eq!(busy.sends, 3);
    assert!(
        matches!(
            &ran[..],
            [Event::Ran {
                step: 1,
                entry: Some(300),
                ..
            }]
        ),
        "{ran:?}"
    );
    let goal = goals.get(id).unwrap();
    assert_eq!((goal.steps_run, goal.failed), (1, 0));
}

#[test]
fn a_busy_step_is_never_sent_past_the_deadline() {
    let mut goals = Goals::new();
    let id = approved(&mut goals);
    let mut busy = Busy { sends: 0, busy: 1 };
    let mut runner = Runner::new();
    turn(&mut runner, &mut goals, T0 + 1, &mut busy);
    let late = turn(&mut runner, &mut goals, T0 + 8 * HOUR + 1, &mut busy);
    assert_eq!(busy.sends, 1, "not sent past the deadline");
    assert!(
        late.iter().any(|e| matches!(
            e,
            Event::Paused {
                pause: Pause::Deadline,
                ..
            }
        )),
        "{late:?}"
    );
    let goal = goals.get(id).unwrap();
    assert_eq!(goal.state, State::Paused(Pause::Deadline));
    assert_eq!(goal.flight, None);
    assert_eq!((goal.steps_run, goal.failed), (0, 0));
    let last = goal.log.last().unwrap();
    assert_eq!(last.status, StepStatus::Held);
    assert_eq!(last.note, "the deadline passed");
}

#[test]
fn a_cancel_while_a_step_is_unsent_logs_it_held_not_failed() {
    let mut goals = Goals::new();
    let id = approved(&mut goals);
    let mut busy = Busy { sends: 0, busy: 1 };
    let mut runner = Runner::new();
    turn(&mut runner, &mut goals, T0 + 1, &mut busy);
    let goal = goals.get_mut(id).unwrap();
    assert!(runner.withdraw_unsent(goal));
    goal.cancel(T0 + 2).unwrap();
    assert_eq!((goal.steps_run, goal.failed), (0, 0));
    let last = goal.log.last().unwrap();
    assert_eq!(last.status, StepStatus::Held);
    assert_eq!(last.note, UNSENT_ENDED);
    // What the goal keeps reads back.
    let text = goals.to_text();
    assert!(Goals::parse(&text).is_ok(), "{text}");
    turn(&mut runner, &mut goals, T0 + 1 + BUSY_FIRST_MS, &mut busy);
    assert_eq!(busy.sends, 1, "a cancelled goal's step is not sent");
}
