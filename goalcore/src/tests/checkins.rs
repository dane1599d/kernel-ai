//! Check-ins: what the goal tells the person, and when.

use super::*;

// ---- check-ins

fn lines(goal: &mut Goal, now: u64) -> Vec<(CheckKind, String)> {
    let mut out = Vec::new();
    while let Some(checkin) = goal.due_checkin(now) {
        out.push((checkin.kind, checkin.line));
    }
    out
}

#[test]
fn a_set_time_posts_one_line_once_and_several_that_passed_are_one_line() {
    let mut goal = goal();
    assert!(lines(&mut goal, T0 + HOUR).is_empty());
    let first = lines(&mut goal, T0 + 2 * HOUR);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].0, CheckKind::Scheduled);
    assert!(lines(&mut goal, T0 + 2 * HOUR + 1).is_empty());
    // Both times have passed by the next look: one line for the one left.
    assert_eq!(lines(&mut goal, T0 + 5 * HOUR).len(), 1);

    let mut together = self::goal();
    let late = lines(&mut together, T0 + 5 * HOUR);
    assert_eq!(late.len(), 1);
    assert_eq!(together.posted.scheduled, 2);
}

#[test]
fn a_line_says_what_is_done_what_is_next_and_what_it_waits_on() {
    let mut goal = goal();
    goal.plan(4, "write reports/q1.txt");
    ran(&mut goal, T0, run_step("fs_list"), 1, 10);
    ran(&mut goal, T0, run_step("fs_read"), 2, 10);
    assert_eq!(
        goal.progress_line(),
        "Goal 'draft the quarterly report': 2 of 4 steps done; next: write reports/q1.txt; waiting on: nothing"
    );
    let p = step("fs_trash", &["reports/old.txt"]);
    goal.propose(T0, p, Presence::Here, &DOOR).unwrap();
    assert_eq!(
        goal.progress_line(),
        "Goal 'draft the quarterly report': 2 of 4 steps done; next: fs_trash reports/old.txt; waiting on: fs_trash has no undo, so it asks"
    );
    goal.deny().unwrap();
    goal.pause().unwrap();
    assert!(
        goal.progress_line()
            .ends_with("waiting on: you, who paused it")
    );
}

#[test]
fn at_75_percent_of_the_time_a_goal_that_is_behind_says_so() {
    // Planned, and not one step run yet: no rate to go by, so only the clock speaks.
    let mut goal = goal();
    goal.plan(4, "next");
    // The marks: the set times first, then 75% of 8 h is 6 h.
    let before = lines(&mut goal, T0 + 5 * HOUR);
    assert!(before.iter().all(|(kind, _)| *kind == CheckKind::Scheduled));
    let at = lines(&mut goal, T0 + 6 * HOUR);
    assert_eq!(at.len(), 1);
    assert_eq!(at[0].0, CheckKind::Behind);
    assert!(
        at[0].1.ends_with("It is behind: 75% of the time has gone"),
        "{}",
        at[0].1
    );
    assert!(lines(&mut goal, T0 + 7 * HOUR).is_empty());
}

#[test]
fn at_75_percent_a_goal_on_pace_posts_no_behind_line() {
    let mut goal = goal();
    goal.plan(4, "last one");
    for entry in 0..3 {
        ran(&mut goal, T0, run_step("fs_list"), entry, 10);
    }
    let at = lines(&mut goal, T0 + 6 * HOUR);
    assert!(
        at.iter().all(|(kind, _)| *kind != CheckKind::Behind),
        "{at:?}"
    );
    assert!(goal.posted.behind_checked);
    // Looked at once: it is not looked at again for being behind later.
    let later = lines(&mut goal, T0 + 7 * HOUR);
    assert!(
        later.iter().all(|(kind, _)| *kind != CheckKind::Behind),
        "{later:?}"
    );
}

#[test]
fn a_goal_that_cannot_finish_in_its_steps_says_so_at_once() {
    let mut goal = goal();
    goal.plan(6, "step 1");
    let said = lines(&mut goal, T0 + 1);
    assert_eq!(said.len(), 1);
    assert_eq!(said[0].0, CheckKind::WontFinish);
    assert!(
        said[0]
            .1
            .ends_with("It cannot finish: 6 steps are left and the budget allows 4"),
        "{}",
        said[0].1
    );
    // Once.
    assert!(lines(&mut goal, T0 + 2).is_empty());
}

#[test]
fn a_goal_that_cannot_finish_in_its_tokens_says_so() {
    let mut goal = goal();
    goal.plan(4, "next");
    ran(&mut goal, T0, run_step("fs_list"), 1, 4_000);
    let said = lines(&mut goal, T0 + 1);
    assert_eq!(said.len(), 1);
    assert!(
        said[0]
            .1
            .ends_with("It cannot finish: about 12000 tokens are needed and 6000 are left"),
        "{}",
        said[0].1
    );
}

#[test]
fn a_goal_that_cannot_finish_in_its_time_says_so_before_the_deadline() {
    let mut goal = goal();
    goal.plan(4, "next");
    ran(&mut goal, T0 + 3 * HOUR, run_step("fs_list"), 1, 10);
    // One step took 3 h; three more need 9 h and 5 h are left. The 2 h check-in
    // has passed too, and comes after.
    let said = lines(&mut goal, T0 + 3 * HOUR);
    let kinds: Vec<CheckKind> = said.iter().map(|(kind, _)| *kind).collect();
    assert_eq!(kinds, vec![CheckKind::WontFinish, CheckKind::Scheduled]);
    assert!(
        said[0]
            .1
            .ends_with("It cannot finish: about 32400 s are needed and 18000 s are left"),
        "{}",
        said[0].1
    );
}

#[test]
fn a_goal_with_nothing_left_to_do_or_no_plan_does_not_say_it_cannot_finish() {
    let mut goal = goal();
    assert!(lines(&mut goal, T0 + 1).is_empty());
    goal.plan(1, "last");
    ran(&mut goal, T0, run_step("fs_list"), 1, 10);
    assert!(lines(&mut goal, T0 + 2).is_empty());
}

#[test]
fn check_ins_come_most_urgent_first_and_none_for_a_goal_not_running() {
    let mut goal = goal();
    goal.plan(6, "x");
    let all = lines(&mut goal, T0 + 6 * HOUR);
    let kinds: Vec<CheckKind> = all.iter().map(|(kind, _)| *kind).collect();
    assert_eq!(
        kinds,
        vec![
            CheckKind::WontFinish,
            CheckKind::Behind,
            CheckKind::Scheduled
        ]
    );

    let mut finished = self::goal();
    finished.complete(T0, "done").unwrap();
    assert!(lines(&mut finished, T0 + 9 * HOUR).is_empty());
    let mut proposed = Goal::from_spec(2, spec(), T0);
    assert!(lines(&mut proposed, T0 + 9 * HOUR).is_empty());
}
