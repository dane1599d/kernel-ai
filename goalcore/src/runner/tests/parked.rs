//! H2-8: a step goalcore's own scope check asks about, in an agent whose only
//! answer path is the shell's card. The goal is parked, never left waiting, and
//! the step never runs.

use super::*;

fn at(row: &str, target: &str) -> Step<&'static str> {
    Step {
        targets: vec![target.to_string()],
        ..step(row)
    }
}

/// `reports/` is the folder `reports`, as the shell's ceiling reads it: no ask.
#[test]
fn a_trailing_slash_inside_the_scope_runs_without_an_ask() {
    let mut goals = Goals::new();
    let id = approved(&mut goals);
    let mut planner = Script(VecDeque::from([
        Plan::Step(at("fs_list", "reports/")),
        Plan::Step(at("fs_list", "./reports/./q3")),
    ]));
    let (mut steps, mut runner) = (Recorded::default(), Runner::new());
    for now in [T0 + 1, T0 + 2] {
        let events = go(
            &mut runner,
            &mut goals,
            now,
            Presence::Here,
            &mut planner,
            &mut steps,
        );
        assert!(matches!(events[0], Event::Ran { .. }), "{events:?}");
    }
    assert_eq!(steps.ran, [(id, "fs_list".into()), (id, "fs_list".into())]);
    // A leading `/` or a `..` is never inside.
    for path in ["/reports", "reports/../etc", "../reports"] {
        assert!(!crate::valid::path_in(&["reports".into()], path), "{path}");
    }
}

/// A row or a path outside the scope: the step is held, never run, and the goal
/// pauses as the person's own pause, so their resume plans again and it holds
/// no active place meanwhile.
#[test]
fn a_step_outside_the_scope_parks_the_goal_and_never_runs() {
    for outside in [at("fs_find", "reports"), at("fs_list", "notes")] {
        let mut goals = Goals::new();
        let id = approved(&mut goals);
        let row = outside.row.clone();
        let mut planner = Script(VecDeque::from([
            Plan::Step(outside),
            Plan::Step(at("fs_list", "reports")),
        ]));
        let (mut steps, mut runner) = (Recorded::default(), Runner::new());
        let events = go(
            &mut runner,
            &mut goals,
            T0 + 1,
            Presence::Here,
            &mut planner,
            &mut steps,
        );
        assert!(matches!(events[0], Event::Waiting { .. }), "{events:?}");
        // What the goals agent does on that event.
        let goal = goals.get_mut(id).unwrap();
        let line = goal.park_ask().expect("parked");
        assert!(line.starts_with("not run, so it paused:"), "{line}");
        assert_eq!(goal.state, State::Paused(Pause::User));
        assert_eq!(goal.log.last().unwrap().status, StepStatus::Held);
        assert_eq!(goal.park_ask(), None, "parked once");
        // Paused, it plans nothing; resumed, it plans again, and the held
        // step is never run.
        go(
            &mut runner,
            &mut goals,
            T0 + 2,
            Presence::Here,
            &mut planner,
            &mut steps,
        );
        assert!(steps.ran.is_empty());
        goals.resume(id).unwrap();
        for now in [T0 + 3, T0 + 4] {
            go(
                &mut runner,
                &mut goals,
                now,
                Presence::Here,
                &mut planner,
                &mut steps,
            );
        }
        assert_eq!(steps.ran, [(id, "fs_list".into())], "{row}");
    }
}

/// The planner's question is parked as before, with the question as next.
#[test]
fn a_question_parks_with_the_question_as_next() {
    let mut goals = Goals::new();
    let id = approved(&mut goals);
    let goal = goals.get_mut(id).unwrap();
    goal.clarify("which reports?").unwrap();
    assert_eq!(
        goal.park_ask().as_deref(),
        Some("waits for your answer: which reports?")
    );
    assert_eq!(goal.state, State::Paused(Pause::User));
}
