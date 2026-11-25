//! A step whose answer waits on the person's card (`Outcome::Later`): it stays
//! in flight and is polled each turn, while the other goals go on, and the
//! person's pause or cancel still holds it.

use super::*;

/// Steps where goal `slow`'s step waits on the card for `polls` asks, then runs;
/// every other goal's runs at once. Each call is recorded.
struct Card {
    calls: Vec<(u64, String)>,
    slow: u64,
    polls: usize,
}

impl Card {
    fn new(slow: u64, polls: usize) -> Self {
        Self {
            calls: Vec::new(),
            slow,
            polls,
        }
    }
}

impl Steps<&'static str> for Card {
    fn run(&mut self, goal: u64, step: &Step<&'static str>) -> Outcome {
        self.calls.push((goal, step.row.clone()));
        let asked = self.calls.iter().filter(|(g, _)| *g == goal).count();
        if goal == self.slow && asked <= self.polls {
            return Outcome::Later;
        }
        Outcome::Done(StepDone {
            entry: Some(200 + self.calls.len() as u64),
            tokens: 0,
            failed: None,
        })
    }
}

fn turn(
    runner: &mut Runner<&'static str>,
    goals: &mut Goals,
    now: u64,
    planner: &mut Script,
    card: &mut Card,
) -> Vec<Event> {
    let mut parts = Parts {
        planner,
        door: &AsksForTrash,
        steps: card,
    };
    runner.turn(goals, now, Presence::Here, &mut parts)
}

#[test]
fn a_step_on_the_card_is_polled_each_turn_and_holds_no_other_goal() {
    let mut goals = Goals::new();
    let a = approved(&mut goals);
    let b = approved(&mut goals);
    // Goal a's step waits two polls; b's steps run at once.
    let mut planner = Script(VecDeque::from([
        Plan::Step(step("fs_list")),
        Plan::Step(step("fs_list")),
        Plan::Step(step("fs_list")),
    ]));
    let mut card = Card::new(a, 2);
    let mut runner = Runner::new();
    // Turn 1: a's step goes to the card, and b's step runs in the same turn.
    let first = turn(&mut runner, &mut goals, T0 + 1, &mut planner, &mut card);
    assert!(
        matches!(&first[..], [Event::Ran { goal, step: 1, .. }] if *goal == b),
        "{first:?}"
    );
    assert_eq!(
        goals.get(a).unwrap().flight,
        Some(1),
        "a's step stays in flight"
    );
    // Turn 2: a is polled, not planned again, and b goes on.
    let second = turn(&mut runner, &mut goals, T0 + 2, &mut planner, &mut card);
    assert!(
        matches!(&second[..], [Event::Ran { goal, step: 2, .. }] if *goal == b),
        "{second:?}"
    );
    // Turn 3: the person said yes on the card; a's step ran.
    let third = turn(&mut runner, &mut goals, T0 + 3, &mut planner, &mut card);
    assert!(
        matches!(&third[0], Event::Ran { goal, step: 1, entry: Some(_) } if *goal == a),
        "{third:?}"
    );
    let a_calls: Vec<_> = card.calls.iter().filter(|(g, _)| *g == a).collect();
    assert_eq!(a_calls.len(), 3, "one send and two polls, one a turn");
    assert!(planner.0.is_empty(), "a was planned once: {:?}", planner.0);
}

#[test]
fn a_pause_holds_a_step_on_the_card_and_a_cancel_drops_it() {
    for cancel in [false, true] {
        let mut goals = Goals::new();
        let id = approved(&mut goals);
        let mut planner = Script(VecDeque::from([Plan::Step(step("fs_list"))]));
        let mut card = Card::new(id, 1);
        let mut runner = Runner::new();
        turn(&mut runner, &mut goals, T0 + 1, &mut planner, &mut card);
        assert_eq!(card.calls.len(), 1);
        // The person's pause or cancel comes through while the card is open.
        let goal = goals.get_mut(id).unwrap();
        match cancel {
            true => goal.cancel(T0 + 2).unwrap(),
            false => goal.pause().unwrap(),
        }
        let events = turn(&mut runner, &mut goals, T0 + 3, &mut planner, &mut card);
        assert!(events.is_empty(), "{events:?}");
        assert_eq!(card.calls.len(), 1, "not polled while held");
        if cancel {
            continue;
        }
        goals.resume(id).unwrap();
        let events = turn(&mut runner, &mut goals, T0 + 4, &mut planner, &mut card);
        assert!(
            matches!(
                &events[0],
                Event::Ran {
                    step: 1,
                    entry: Some(_),
                    ..
                }
            ),
            "{events:?}"
        );
        assert_eq!(card.calls.len(), 2, "polled again after the resume");
    }
}
