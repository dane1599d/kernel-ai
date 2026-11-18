//! The registry's storm rules.

use super::*;

// ---- the registry's storm rules

#[test]
fn a_request_is_held_to_the_limits_before_it_is_a_goal() {
    let mut goals = Goals::new();
    type Change = fn(&mut Spec);
    let bad: Vec<(&str, Change)> = vec![
        ("empty words", |s: &mut Spec| s.text.clear()),
        ("padded words", |s: &mut Spec| s.text = String::from(" x")),
        ("tab in words", |s: &mut Spec| s.text = String::from("a\tb")),
        ("long words", |s: &mut Spec| {
            s.text = "x".repeat(MAX_TEXT + 1)
        }),
        ("no rows", |s: &mut Spec| s.scope.rows.clear()),
        ("a bad row", |s: &mut Spec| {
            s.scope.rows = strings(&["Fs_list"])
        }),
        ("a bare star", |s: &mut Spec| s.scope.rows = strings(&["*"])),
        ("a row twice", |s: &mut Spec| {
            s.scope.rows = strings(&["fs_list", "fs_list"])
        }),
        ("too many rows", |s: &mut Spec| {
            s.scope.rows = (0..=MAX_ROWS)
                .map(|n| format!("row_{}", "a".repeat(n + 1)))
                .collect();
        }),
        ("a folder that leaves home", |s: &mut Spec| {
            s.scope.folders = strings(&["../etc"])
        }),
        ("an absolute folder", |s: &mut Spec| {
            s.scope.folders = strings(&["/home/user"])
        }),
        ("a folder with a hole", |s: &mut Spec| {
            s.scope.folders = strings(&["a//b"])
        }),
        ("zero steps", |s: &mut Spec| s.budget.steps = 0),
        ("too many steps", |s: &mut Spec| {
            s.budget.steps = MAX_STEPS + 1
        }),
        ("zero tokens", |s: &mut Spec| s.budget.tokens = 0),
        ("zero time", |s: &mut Spec| s.budget.wall_ms = 0),
        ("a deadline in the past", |s: &mut Spec| s.by_ms = Some(T0)),
        ("check-ins in the past", |s: &mut Spec| {
            s.checkins = vec![T0]
        }),
        ("check-ins out of order", |s: &mut Spec| {
            s.checkins = vec![T0 + 2, T0 + 1]
        }),
        ("a check-in after the deadline", |s: &mut Spec| {
            s.checkins = vec![T0 + 9 * HOUR]
        }),
    ];
    for (what, change) in bad {
        let mut request = spec();
        change(&mut request);
        assert!(
            matches!(goals.set(request, T0), Err(Refusal::BadSpec(_))),
            "{what} should be refused"
        );
    }
    assert!(goals.list().is_empty());
    assert!(goals.set(spec(), T0).is_ok());
}

#[test]
fn at_most_four_goals_are_active_and_pausing_one_makes_room() {
    let mut goals = Goals::new();
    let ids: Vec<u64> = (0..5).map(|_| goals.set(spec(), T0).unwrap()).collect();
    for id in &ids[..4] {
        goals.approve(*id, T0).unwrap();
    }
    assert_eq!(goals.approve(ids[4], T0), Err(Refusal::TooManyActive));
    assert_eq!(goals.live(), 4);
    goals.get_mut(ids[0]).unwrap().pause().unwrap();
    goals.approve(ids[4], T0).unwrap();
    // The paused one cannot resume into a full house.
    assert_eq!(goals.resume(ids[0]), Err(Refusal::TooManyActive));
    goals.get_mut(ids[1]).unwrap().cancel(T0).unwrap();
    assert_eq!(goals.resume(ids[0]), Ok(()));
}

#[test]
fn a_waiting_goal_counts_as_active() {
    let mut goals = Goals::new();
    let ids: Vec<u64> = (0..5).map(|_| goals.set(spec(), T0).unwrap()).collect();
    for id in &ids[..4] {
        goals.approve(*id, T0).unwrap();
    }
    let p = step("fs_trash", &["reports/a"]);
    goals
        .get_mut(ids[0])
        .unwrap()
        .propose(T0, p, Presence::Here, &DOOR)
        .unwrap();
    assert_eq!(goals.live(), 4);
    assert_eq!(goals.approve(ids[4], T0), Err(Refusal::TooManyActive));
}

#[test]
fn at_most_sixteen_goals_are_kept_and_only_a_finished_one_is_forgotten() {
    let mut goals = Goals::new();
    let ids: Vec<u64> = (0..MAX_GOALS)
        .map(|_| goals.set(spec(), T0).unwrap())
        .collect();
    assert_eq!(goals.set(spec(), T0), Err(Refusal::TooManyGoals));
    assert_eq!(
        goals.forget(ids[0]),
        Err(Refusal::NotFinished(State::Proposed))
    );
    goals.get_mut(ids[0]).unwrap().cancel(T0).unwrap();
    goals.forget(ids[0]).unwrap();
    assert_eq!(goals.forget(ids[0]), Err(Refusal::NoSuchGoal(ids[0])));
    let next = goals.set(spec(), T0).unwrap();
    // Numbers are never used twice.
    assert!(next > ids[MAX_GOALS - 1]);
}

#[test]
fn a_tick_pauses_a_goal_past_its_deadline_unless_a_step_runs_and_posts_check_ins() {
    let mut goals = Goals::new();
    let (a, b, c) = (
        goals.set(spec(), T0).unwrap(),
        goals.set(spec(), T0).unwrap(),
        goals.set(spec(), T0).unwrap(),
    );
    for id in [a, b, c] {
        goals.approve(id, T0).unwrap();
    }
    goals
        .get_mut(b)
        .unwrap()
        .propose(T0, run_step("fs_list"), Presence::Here, &DOOR)
        .unwrap();
    let p = step("fs_trash", &["reports/a"]);
    goals
        .get_mut(c)
        .unwrap()
        .propose(T0, p, Presence::Here, &DOOR)
        .unwrap();
    let posted = goals.tick(T0 + 9 * HOUR);
    assert_eq!(goals.get(a).unwrap().state, State::Paused(Pause::Deadline));
    assert_eq!(goals.get(b).unwrap().state, State::Active);
    assert_eq!(goals.get(c).unwrap().state, State::Waiting);
    // Each goal gave one line: 75% of the way to the deadline has gone, and none
    // has a plan, so none is on pace. The finished and paused give the same.
    let who: Vec<u64> = posted.iter().map(|(id, _)| *id).collect();
    assert_eq!(who, vec![a, b, c]);
}
