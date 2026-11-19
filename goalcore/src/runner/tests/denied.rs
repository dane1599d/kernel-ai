//! A no is remembered (#1066's LOW): the step the person said no to is not asked
//! a second time, whichever way they said it.

use super::*;

fn step_on(row: &str, path: &str) -> Step<&'static str> {
    Step {
        targets: vec![path.to_string()],
        ..step(row)
    }
}

/// #1066's LOW: a planner that writes the denied step again does not put it on the
/// card again. The goal pauses; the person's resume lets it be asked once more, and
/// a second no pauses it at the next repeat in the same way.
#[test]
fn a_step_the_person_said_no_to_is_not_asked_again() {
    let mut goals = Goals::new();
    let id = approved(&mut goals);
    let mut planner = Script(VecDeque::from([
        Plan::Step(step("fs_list")),
        Plan::Step(step("fs_list")),
        Plan::Step(step("fs_list")),
        Plan::Step(step("fs_list")),
    ]));
    let mut steps = Recorded {
        deny: vec!["fs_list"],
        ..Recorded::default()
    };
    let mut runner = Runner::new();
    let repeated = [Event::Repeated {
        goal: id,
        row: "fs_list".to_string(),
    }];
    let mut turn = |goals: &mut Goals, steps: &mut Recorded, now| {
        go(&mut runner, goals, now, Presence::Away, &mut planner, steps)
    };
    assert_eq!(
        turn(&mut goals, &mut steps, T0 + 1),
        [Event::Denied { goal: id, step: 1 }]
    );
    // The same step again: not sent to the card, and the goal stops.
    assert_eq!(turn(&mut goals, &mut steps, T0 + 2), repeated);
    assert_eq!(steps.ran.len(), 1, "asked once");
    let goal = goals.get(id).unwrap();
    assert_eq!(goal.state, State::Paused(Pause::User));
    assert_eq!((goal.log.len(), goal.asked, goal.denied), (1, 1, 1));
    // Paused, it is not planned: nothing is asked while the person is silent.
    assert_eq!(turn(&mut goals, &mut steps, T0 + 3), []);
    assert_eq!(steps.ran.len(), 1);
    // The person resumes it: the step may be asked once more, and no more than that.
    goals.resume(id).unwrap();
    assert_eq!(
        turn(&mut goals, &mut steps, T0 + 4),
        [Event::Denied { goal: id, step: 2 }]
    );
    assert_eq!(turn(&mut goals, &mut steps, T0 + 5), repeated);
    assert_eq!(
        steps.ran.len(),
        2,
        "one card for each time the person acted"
    );
    assert_eq!(goals.get(id).unwrap().state, State::Paused(Pause::User));
    let text = goals.to_text();
    assert_eq!(Goals::parse(&text).as_ref(), Ok(&goals), "{text}");
}

/// A no is to one step: another path or another call is asked as usual, and every
/// step denied is remembered, not only the last.
#[test]
fn a_no_is_to_that_step_and_every_denied_step_is_remembered() {
    let mut goals = Goals::new();
    approved(&mut goals);
    let other_call = Step {
        call: "another call",
        ..step("fs_list")
    };
    let mut planner = Script(VecDeque::from([
        Plan::Step(step("fs_list")),
        Plan::Step(step_on("fs_list", "reports/q3")),
        Plan::Step(other_call),
        Plan::Step(Step {
            tokens_est: 99,
            ..step("fs_list")
        }),
    ]));
    let mut steps = Recorded {
        deny: vec!["fs_list"],
        ..Recorded::default()
    };
    let mut runner = Runner::new();
    for at in 1..=3 {
        let events = go(
            &mut runner,
            &mut goals,
            T0 + at,
            Presence::Away,
            &mut planner,
            &mut steps,
        );
        assert!(matches!(events[..], [Event::Denied { .. }]), "{events:?}");
    }
    assert_eq!(steps.ran.len(), 3, "three steps, three cards");
    // The first one again, two denied steps later and with another estimate.
    let events = go(
        &mut runner,
        &mut goals,
        T0 + 4,
        Presence::Away,
        &mut planner,
        &mut steps,
    );
    assert!(matches!(events[..], [Event::Repeated { .. }]), "{events:?}");
    assert_eq!(steps.ran.len(), 3);
}

/// The no where goalcore itself asked (`Goal::deny`, on the trusted path) is
/// remembered the same way: the step does not wait for the person a second time.
#[test]
fn a_step_denied_while_it_waited_is_not_asked_again() {
    let mut goals = Goals::new();
    let id = approved(&mut goals);
    let mut planner = Script(VecDeque::from([
        Plan::Step(step("fs_trash")),
        Plan::Step(step("fs_trash")),
    ]));
    let (mut steps, mut runner) = (Recorded::default(), Runner::new());
    let mut turn = |goals: &mut Goals, steps: &mut Recorded, now| {
        go(&mut runner, goals, now, Presence::Here, &mut planner, steps)
    };
    let asked = turn(&mut goals, &mut steps, T0 + 1);
    assert!(matches!(asked[..], [Event::Waiting { .. }]), "{asked:?}");
    goals.get_mut(id).unwrap().deny().unwrap();
    let events = turn(&mut goals, &mut steps, T0 + 2);
    assert_eq!(
        events,
        [Event::Repeated {
            goal: id,
            row: "fs_trash".to_string()
        }]
    );
    let goal = goals.get(id).unwrap();
    assert_eq!(goal.state, State::Paused(Pause::User));
    assert_eq!((goal.asked, goal.denied), (1, 1), "it waited once");
    assert!(steps.ran.is_empty());
}

/// What the runner holds against a goal goes when the goal ends.
#[test]
fn a_goal_that_ends_takes_its_denied_steps_with_it() {
    let mut goals = Goals::new();
    let id = approved(&mut goals);
    let mut planner = Script(VecDeque::from([Plan::Step(step("fs_list"))]));
    let mut steps = Recorded {
        deny: vec!["fs_list"],
        ..Recorded::default()
    };
    let mut runner = Runner::new();
    go(
        &mut runner,
        &mut goals,
        T0 + 1,
        Presence::Away,
        &mut planner,
        &mut steps,
    );
    assert_eq!(runner.denied.get(&id).map(Vec::len), Some(1));
    goals.get_mut(id).unwrap().cancel(T0 + 2).unwrap();
    go(
        &mut runner,
        &mut goals,
        T0 + 3,
        Presence::Away,
        &mut planner,
        &mut steps,
    );
    assert!(runner.denied.is_empty());
}
