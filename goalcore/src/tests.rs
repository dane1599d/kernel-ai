//! The goal rules, run against a fake clock.

use super::*;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

const HOUR: u64 = 3_600_000;
const T0: u64 = 1_000_000;

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

fn spec() -> Spec {
    Spec {
        text: String::from("draft the quarterly report"),
        by_ms: Some(T0 + 8 * HOUR),
        scope: Scope {
            rows: strings(&["fs_list", "fs_read", "fs_create", "fs_trash", "notes_*"]),
            folders: strings(&["reports"]),
        },
        budget: Budget {
            steps: 4,
            tokens: 10_000,
            wall_ms: 10 * HOUR,
        },
        checkins: vec![T0 + 2 * HOUR, T0 + 4 * HOUR],
        away_ok: true,
    }
}

/// An approved goal, started at `T0`.
fn goal() -> Goal {
    let mut goal = Goal::from_spec(1, spec(), T0);
    goal.start(T0).unwrap();
    goal
}

/// The one door, as the registry decides tiers: a row with no undo asks.
struct FakeDoor;

impl Door for FakeDoor {
    fn tier(&self, row: &str, _targets: &[String]) -> Tier {
        match row {
            "fs_trash" | "service_stop" | "process_stop" => Tier::Ask,
            _ => Tier::Run,
        }
    }
}

const DOOR: FakeDoor = FakeDoor;

fn step(row: &str, targets: &[&str]) -> Step {
    Step {
        row: row.to_string(),
        targets: strings(targets),
        tokens_est: 100,
    }
}

fn run_step(row: &str) -> Step {
    step(row, &[])
}

fn done(entry: u64, tokens: u64) -> StepDone {
    StepDone {
        entry: Some(entry),
        tokens,
        failed: None,
    }
}

/// Proposes `p` and finishes it ok at `now`, as the runner does for a `Run`.
fn ran(goal: &mut Goal, now: u64, p: Step, entry: u64, tokens: u64) {
    assert_eq!(
        goal.propose(now, p, Presence::Here, &DOOR),
        Ok(Verdict::Run)
    );
    goal.finish_step(done(entry, tokens)).unwrap();
}

// ---- approval and the scope

#[test]
fn a_goal_takes_no_step_until_the_person_approves_it() {
    let mut goals = Goals::new();
    let id = goals.set(spec(), T0).unwrap();
    let proposed = goals.get_mut(id).unwrap();
    assert_eq!(proposed.state, State::Proposed);
    assert_eq!(
        proposed.propose(T0, run_step("fs_list"), Presence::Here, &DOOR),
        Err(Refusal::NotActive(State::Proposed))
    );
    assert!(proposed.log.is_empty());
    goals.approve(id, T0 + 5).unwrap();
    let goal = goals.get(id).unwrap();
    assert_eq!((goal.state, goal.started_ms), (State::Active, Some(T0 + 5)));
}

#[test]
fn the_ask_shows_the_scope_the_budget_and_the_check_ins() {
    let text = spec().approval();
    for needed in [
        "Goal: draft the quarterly report",
        "Rows: fs_list, fs_read, fs_create, fs_trash, notes_*",
        "Folders: reports",
        "Budget: 4 steps, 10000 tokens, 36000 s",
        "Check-ins: 8200000 ms, 15400000 ms",
        "keep going while you are away",
    ] {
        assert!(text.contains(needed), "{needed:?} is not in {text:?}");
    }
}

#[test]
fn a_run_tier_step_in_scope_runs_and_is_logged_with_its_entry_and_tokens() {
    let mut goal = goal();
    let p = step("fs_create", &["reports/q1.txt"]);
    assert_eq!(
        goal.propose(T0 + 10, p.clone(), Presence::Here, &DOOR),
        Ok(Verdict::Run)
    );
    assert_eq!(goal.flight, Some(1));
    assert_eq!(goal.log[0].status, StepStatus::Running);
    goal.finish_step(done(41, 900)).unwrap();
    assert_eq!(
        (goal.steps_run, goal.steps_ok, goal.tokens_spent),
        (1, 1, 900)
    );
    let log = &goal.log[0];
    assert_eq!(log.n, 1);
    assert_eq!(log.at_ms, T0 + 10);
    let logged = Proposal {
        row: p.row,
        targets: p.targets,
        tier: Tier::Run,
        tokens_est: p.tokens_est,
    };
    assert_eq!(log.proposal, logged);
    assert_eq!(
        (log.status, log.entry, log.tokens),
        (StepStatus::Ran, Some(41), 900)
    );
    assert_eq!(goal.flight, None);
}

#[test]
fn a_run_tier_step_runs_while_the_person_is_away() {
    let mut goal = goal();
    let verdict = goal.propose(T0, run_step("fs_list"), Presence::Away, &DOOR);
    assert_eq!(verdict, Ok(Verdict::Run));
}

#[test]
fn a_goal_that_does_not_run_unattended_waits_for_the_person_to_return() {
    let mut goal = goal();
    goal.away_ok = false;
    let Ok(Verdict::Ask(ask)) = goal.propose(T0, run_step("fs_list"), Presence::Away, &DOOR) else {
        panic!("it should ask");
    };
    assert_eq!(ask.kind, AskKind::Away);
    assert_eq!(goal.state, State::Waiting);
    // Back at the machine, the same step runs without a word.
    goal.deny().unwrap();
    assert_eq!(
        goal.propose(T0, run_step("fs_list"), Presence::Here, &DOOR),
        Ok(Verdict::Run)
    );
}

#[test]
fn only_one_step_runs_at_a_time() {
    let mut goal = goal();
    assert_eq!(
        goal.propose(T0, run_step("fs_list"), Presence::Here, &DOOR),
        Ok(Verdict::Run)
    );
    assert_eq!(
        goal.propose(T0, run_step("fs_read"), Presence::Here, &DOOR),
        Err(Refusal::StepInFlight)
    );
    assert_eq!(goal.log.len(), 1);
    assert_eq!(goal.complete(T0, "done"), Err(Refusal::StepInFlight));
}

#[test]
fn a_step_that_asks_pauses_the_goal_until_the_person_answers() {
    let mut goal = goal();
    let p = step("fs_trash", &["reports/old.txt"]);
    let Ok(Verdict::Ask(ask)) = goal.propose(T0, p, Presence::Here, &DOOR) else {
        panic!("it should ask");
    };
    assert_eq!(ask.kind, AskKind::Tier);
    assert_eq!(goal.state, State::Waiting);
    assert_eq!(
        goal.waiting_line().as_deref(),
        Some(
            "Goal 'draft the quarterly report' is waiting for you: fs_trash has no undo, so it asks"
        )
    );
    assert_eq!(goal.log[0].status, StepStatus::Asked);
    // Nothing else runs while it waits.
    assert_eq!(
        goal.propose(T0, run_step("fs_list"), Presence::Here, &DOOR),
        Err(Refusal::NotActive(State::Waiting))
    );
    // A yes runs that one step.
    assert_eq!(goal.approve(T0 + 1), Ok(Verdict::Run));
    assert_eq!((goal.state, goal.flight), (State::Active, Some(1)));
    assert_eq!(goal.log[0].status, StepStatus::Running);
    goal.finish_step(done(7, 50)).unwrap();
    assert_eq!(goal.log[0].status, StepStatus::Ran);
    assert!(goal.waiting_line().is_none());
}

#[test]
fn a_no_drops_the_step_and_the_goal_goes_on() {
    let mut goal = goal();
    let p = step("fs_trash", &["reports/old.txt"]);
    assert!(matches!(
        goal.propose(T0, p, Presence::Here, &DOOR),
        Ok(Verdict::Ask(_))
    ));
    goal.deny().unwrap();
    assert_eq!((goal.state, goal.denied), (State::Active, 1));
    assert_eq!(goal.log[0].status, StepStatus::Denied);
    assert_eq!(goal.steps_run, 0);
    assert_eq!(goal.deny(), Err(Refusal::NothingWaiting));
    assert_eq!(goal.approve(T0), Err(Refusal::NothingWaiting));
    assert_eq!(
        goal.propose(T0, run_step("fs_list"), Presence::Here, &DOOR),
        Ok(Verdict::Run)
    );
}

#[test]
fn a_step_outside_the_scope_asks_whatever_its_tier_and_a_yes_does_not_widen_it() {
    let mut goal = goal();
    let outside = run_step("service_restart");
    let Ok(Verdict::Ask(ask)) = goal.propose(T0, outside.clone(), Presence::Here, &DOOR) else {
        panic!("it should ask");
    };
    assert_eq!(ask.kind, AskKind::Scope);
    assert_eq!(ask.why, "service_restart is outside what you approved");
    assert_eq!(goal.approve(T0), Ok(Verdict::Run));
    goal.finish_step(done(3, 10)).unwrap();
    assert_eq!(goal.scope, spec().scope);
    // The same step asks again.
    assert!(matches!(
        goal.propose(T0, outside, Presence::Here, &DOOR),
        Ok(Verdict::Ask(_))
    ));
}

#[test]
fn nothing_outside_the_scope_runs_unasked() {
    let rows = [
        "service_restart",
        "process_stop",
        "fs_move",
        "notes",
        "note_add",
        "x",
    ];
    let paths = [
        "",
        "a.txt",
        "reports/../a.txt",
        "/home/user/reports/a.txt",
        "reportsx/a",
    ];
    for row in rows {
        let mut goal = goal();
        let verdict = goal.propose(T0, run_step(row), Presence::Here, &DOOR);
        assert!(
            matches!(verdict, Ok(Verdict::Ask(ref a)) if a.kind == AskKind::Scope),
            "{row}"
        );
    }
    for path in paths {
        let mut goal = goal();
        let p = step("fs_create", &[path]);
        let verdict = goal.propose(T0, p, Presence::Here, &DOOR);
        let outside = matches!(verdict, Ok(Verdict::Ask(ref a)) if a.kind == AskKind::Scope);
        // An empty path cannot be logged at all; the others are outside.
        assert!(
            outside || matches!(verdict, Err(Refusal::BadProposal(_))),
            "{path:?}"
        );
    }
}

#[test]
fn a_path_inside_an_approved_folder_runs_and_the_folder_itself_does_too() {
    for path in ["reports", "reports/q1.txt", "reports/2026/q1.txt"] {
        let mut goal = goal();
        let p = step("fs_list", &[path]);
        assert_eq!(
            goal.propose(T0, p, Presence::Here, &DOOR),
            Ok(Verdict::Run),
            "{path}"
        );
    }
}

#[test]
fn a_goal_with_no_folders_may_touch_no_path_but_a_step_with_none_runs() {
    let mut goal = goal();
    goal.scope.folders.clear();
    let with_path = step("fs_list", &["reports"]);
    assert!(matches!(
        goal.propose(T0, with_path, Presence::Here, &DOOR),
        Ok(Verdict::Ask(_))
    ));
    goal.deny().unwrap();
    assert_eq!(
        goal.propose(T0, run_step("fs_list"), Presence::Here, &DOOR),
        Ok(Verdict::Run)
    );
}

#[test]
fn a_group_in_the_scope_covers_its_rows_only() {
    let mut goal = goal();
    ran(&mut goal, T0, run_step("notes_add"), 1, 10);
    assert!(matches!(
        goal.propose(T0, run_step("notesx_add"), Presence::Here, &DOOR),
        Ok(Verdict::Ask(_))
    ));
}

#[test]
fn a_step_the_log_cannot_keep_is_refused_and_not_logged() {
    let mut goal = goal();
    for bad in [
        run_step("Fs List"),
        run_step(""),
        step("fs_list", &[""]),
        step("fs_list", &["a", "b", "c", "d", "e"]),
        step("fs_list", &["tab\there"]),
    ] {
        assert!(matches!(
            goal.propose(T0, bad, Presence::Here, &DOOR),
            Err(Refusal::BadProposal(_))
        ));
    }
    assert!(goal.log.is_empty());
}

#[test]
fn a_goal_whose_log_is_full_takes_no_more_steps() {
    let mut goal = goal();
    goal.budget.steps = 64;
    goal.budget.tokens = MAX_TOKENS;
    for _ in 0..MAX_LOG {
        ran(&mut goal, T0, run_step("fs_list"), 1, 0);
        goal.budget.steps = 64;
        goal.steps_run = 0;
    }
    assert_eq!(
        goal.propose(T0, run_step("fs_list"), Presence::Here, &DOOR),
        Err(Refusal::LogFull)
    );
}

// ---- budget and deadline

#[test]
fn at_the_step_budget_the_goal_pauses_and_a_raise_resumes_it() {
    let mut goals = Goals::new();
    let id = goals.set(spec(), T0).unwrap();
    goals.approve(id, T0).unwrap();
    for n in 0..4 {
        ran(
            goals.get_mut(id).unwrap(),
            T0 + n,
            run_step("fs_list"),
            n,
            10,
        );
    }
    let goal = goals.get_mut(id).unwrap();
    let verdict = goal.propose(T0 + 9, run_step("fs_list"), Presence::Here, &DOOR);
    assert_eq!(verdict, Ok(Verdict::Over(Pause::Budget(Resource::Steps))));
    assert_eq!(goal.state, State::Paused(Pause::Budget(Resource::Steps)));
    assert_eq!(goal.log.last().unwrap().status, StepStatus::Held);
    assert_eq!(goal.log.last().unwrap().note, "the steps budget is used up");
    // Resume needs the raise, not the person's resume.
    assert_eq!(
        goals.resume(id),
        Err(Refusal::NeedsRaise(Pause::Budget(Resource::Steps)))
    );
    let more = Budget {
        steps: 2,
        tokens: 0,
        wall_ms: 0,
    };
    assert_eq!(goals.raise(id, T0 + 10, more), Ok(State::Active));
    let goal = goals.get_mut(id).unwrap();
    assert_eq!(goal.budget.steps, 6);
    assert_eq!(
        goal.propose(T0 + 11, run_step("fs_list"), Presence::Here, &DOOR),
        Ok(Verdict::Run)
    );
}

#[test]
fn a_step_that_would_pass_the_token_budget_pauses_the_goal() {
    let mut goal = goal();
    ran(&mut goal, T0, run_step("fs_list"), 1, 9_950);
    let verdict = goal.propose(T0, run_step("fs_list"), Presence::Here, &DOOR);
    assert_eq!(verdict, Ok(Verdict::Over(Pause::Budget(Resource::Tokens))));
}

#[test]
fn planning_tokens_count_against_the_budget_too() {
    let mut goal = goal();
    goal.charge(9_990);
    let verdict = goal.propose(T0, run_step("fs_list"), Presence::Here, &DOOR);
    assert_eq!(verdict, Ok(Verdict::Over(Pause::Budget(Resource::Tokens))));
}

#[test]
fn past_the_wall_time_budget_the_goal_pauses() {
    let mut goal = goal();
    goal.budget.wall_ms = HOUR;
    let verdict = goal.propose(T0 + HOUR, run_step("fs_list"), Presence::Here, &DOOR);
    assert_eq!(verdict, Ok(Verdict::Over(Pause::Budget(Resource::Wall))));
}

#[test]
fn a_raise_that_still_leaves_it_short_keeps_it_paused() {
    let mut goals = Goals::new();
    let id = goals.set(spec(), T0).unwrap();
    goals.approve(id, T0).unwrap();
    goals.get_mut(id).unwrap().charge(10_000);
    let verdict =
        goals
            .get_mut(id)
            .unwrap()
            .propose(T0, run_step("fs_list"), Presence::Here, &DOOR);
    assert!(matches!(verdict, Ok(Verdict::Over(_))));
    let tiny = Budget {
        steps: 0,
        tokens: 0,
        wall_ms: 0,
    };
    let state = goals.raise(id, T0, tiny).unwrap();
    assert_eq!(state, State::Paused(Pause::Budget(Resource::Tokens)));
}

#[test]
fn at_the_deadline_the_goal_pauses_and_a_later_one_resumes_it() {
    let mut goals = Goals::new();
    let id = goals.set(spec(), T0).unwrap();
    goals.approve(id, T0).unwrap();
    let by = T0 + 8 * HOUR;
    let goal = goals.get_mut(id).unwrap();
    let verdict = goal.propose(by, run_step("fs_list"), Presence::Here, &DOOR);
    assert_eq!(verdict, Ok(Verdict::Over(Pause::Deadline)));
    assert_eq!(goals.resume(id), Err(Refusal::NeedsRaise(Pause::Deadline)));
    // Not later than the old one, or than now: refused.
    assert!(matches!(goals.extend(id, by, by), Err(Refusal::BadSpec(_))));
    assert_eq!(goals.extend(id, by, by + HOUR), Ok(State::Active));
    let goal = goals.get_mut(id).unwrap();
    assert_eq!(
        goal.propose(by + 1, run_step("fs_list"), Presence::Here, &DOOR),
        Ok(Verdict::Run)
    );
}

#[test]
fn a_budget_that_ran_out_while_a_step_waited_holds_the_step_back() {
    let mut goal = goal();
    goal.budget.wall_ms = HOUR;
    let p = step("fs_trash", &["reports/old.txt"]);
    assert!(matches!(
        goal.propose(T0, p, Presence::Here, &DOOR),
        Ok(Verdict::Ask(_))
    ));
    // The person answers after the time ran out.
    let verdict = goal.approve(T0 + HOUR + 1);
    assert_eq!(verdict, Ok(Verdict::Over(Pause::Budget(Resource::Wall))));
    assert_eq!(goal.log[0].status, StepStatus::Held);
    assert_eq!(goal.flight, None);
}

// ---- pause, finish, cancel, undo

#[test]
fn the_person_can_pause_and_resume_and_a_running_step_still_finishes() {
    let mut goals = Goals::new();
    let id = goals.set(spec(), T0).unwrap();
    goals.approve(id, T0).unwrap();
    let goal = goals.get_mut(id).unwrap();
    assert_eq!(
        goal.propose(T0, run_step("fs_list"), Presence::Here, &DOOR),
        Ok(Verdict::Run)
    );
    goal.pause().unwrap();
    assert_eq!(goal.state, State::Paused(Pause::User));
    goal.finish_step(done(2, 10)).unwrap();
    assert_eq!(
        goal.propose(T0, run_step("fs_list"), Presence::Here, &DOOR),
        Err(Refusal::NotActive(State::Paused(Pause::User)))
    );
    assert_eq!(goals.resume(id), Ok(()));
    assert_eq!(goals.resume(id), Err(Refusal::NotPaused(State::Active)));
}

#[test]
fn a_failed_step_counts_against_the_budget_and_is_logged_with_why() {
    let mut goal = goal();
    assert_eq!(
        goal.propose(T0, run_step("fs_read"), Presence::Here, &DOOR),
        Ok(Verdict::Run)
    );
    let failed = StepDone {
        entry: Some(9),
        tokens: 20,
        failed: Some(String::from("no file\nreports/q1.txt\u{7f}")),
    };
    goal.finish_step(failed).unwrap();
    assert_eq!((goal.steps_run, goal.steps_ok, goal.failed), (1, 0, 1));
    let log = &goal.log[0];
    assert_eq!(log.status, StepStatus::Failed);
    assert_eq!(log.note, "no file?reports/q1.txt?");
    assert_eq!(goal.finish_step(done(1, 1)), Err(Refusal::NoStepRunning));
}

#[test]
fn complete_fail_and_cancel_end_the_goal_and_close_what_was_open() {
    let mut goal = goal();
    goal.complete(T0 + 5, "the report is in reports/q1.txt")
        .unwrap();
    assert_eq!((goal.state, goal.ended_ms), (State::Done, Some(T0 + 5)));
    assert_eq!(goal.note, "the report is in reports/q1.txt");
    assert_eq!(goal.cancel(T0), Err(Refusal::Finished(State::Done)));
    assert_eq!(goal.fail(T0, "x"), Err(Refusal::Finished(State::Done)));

    let mut waiting = self::goal();
    let p = step("fs_trash", &["reports/old.txt"]);
    waiting.propose(T0, p, Presence::Here, &DOOR).unwrap();
    waiting.cancel(T0 + 1).unwrap();
    assert_eq!(waiting.state, State::Cancelled);
    assert_eq!(waiting.log[0].status, StepStatus::Held);
    assert!(waiting.waiting.is_none());

    let mut running = self::goal();
    running
        .propose(T0, run_step("fs_list"), Presence::Here, &DOOR)
        .unwrap();
    running.fail(T0 + 2, "the model gave up").unwrap();
    assert_eq!((running.state, running.flight), (State::Failed, None));
    assert_eq!(running.log[0].status, StepStatus::Failed);
}

#[test]
fn a_goal_not_yet_approved_can_be_declined() {
    let mut goals = Goals::new();
    let id = goals.set(spec(), T0).unwrap();
    goals.get_mut(id).unwrap().cancel(T0).unwrap();
    assert_eq!(
        goals.approve(id, T0),
        Err(Refusal::NotProposed(State::Cancelled))
    );
}

#[test]
fn every_entry_a_goal_made_is_there_to_undo_newest_first() {
    let mut goal = goal();
    ran(&mut goal, T0, run_step("fs_create"), 11, 10);
    ran(&mut goal, T0, run_step("fs_create"), 12, 10);
    assert!(matches!(
        goal.propose(T0, run_step("service_restart"), Presence::Here, &DOOR),
        Ok(Verdict::Ask(_))
    ));
    goal.deny().unwrap();
    ran(&mut goal, T0, run_step("fs_create"), 15, 10);
    assert_eq!(goal.undo_entries(), vec![15, 12, 11]);
}

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

// ---- a whole run on the fake clock

#[test]
fn a_goal_runs_its_plan_while_the_person_is_away_and_waits_only_for_the_one_ask() {
    let mut goals = Goals::new();
    let id = goals.set(spec(), T0).unwrap();
    goals.approve(id, T0).unwrap();
    let goal = goals.get_mut(id).unwrap();
    goal.plan(4, "list the reports folder");
    let mut now = T0;
    let mut entry = 100;
    // Three run-tier steps, the person away.
    for (row, path) in [
        ("fs_list", "reports"),
        ("fs_read", "reports/q1.txt"),
        ("fs_create", "reports/summary.txt"),
    ] {
        now += 10_000;
        entry += 1;
        let verdict = goal.propose(now, step(row, &[path]), Presence::Away, &DOOR);
        assert_eq!(verdict, Ok(Verdict::Run), "{row}");
        goal.finish_step(done(entry, 800)).unwrap();
    }
    // The fourth has no undo: it waits, and nothing else runs.
    now += 10_000;
    let p = step("fs_trash", &["reports/old.txt"]);
    assert!(matches!(
        goal.propose(now, p, Presence::Away, &DOOR),
        Ok(Verdict::Ask(_))
    ));
    assert_eq!(goal.state, State::Waiting);
    // The person is back and says yes.
    now += HOUR;
    assert_eq!(goal.approve(now), Ok(Verdict::Run));
    goal.finish_step(done(200, 300)).unwrap();
    goal.complete(now, "summary written, old report trashed")
        .unwrap();
    assert_eq!(goal.state, State::Done);
    assert_eq!(
        (goal.steps_run, goal.denied, goal.tokens_spent),
        (4, 0, 2_700)
    );
    assert_eq!(goal.undo_entries(), vec![200, 103, 102, 101]);
    let statuses: Vec<StepStatus> = goal.log.iter().map(|s| s.status).collect();
    assert_eq!(statuses, vec![StepStatus::Ran; 4]);
}

// ---- more than one path, the door's tier, a question, and "met"

fn mover() -> Goal {
    let mut goal = goal();
    goal.scope.rows.push(String::from("fs_move"));
    goal.scope.folders = strings(&["inbox", "archive"]);
    goal
}

#[test]
fn a_move_names_both_paths_and_both_must_lie_in_the_scope() {
    let mut goal = mover();
    let both = step("fs_move", &["inbox/a.txt", "archive/a.txt"]);
    assert_eq!(
        goal.propose(T0, both, Presence::Here, &DOOR),
        Ok(Verdict::Run)
    );
    assert_eq!(
        goal.log[0].proposal.targets,
        strings(&["inbox/a.txt", "archive/a.txt"])
    );
    goal.finish_step(done(1, 10)).unwrap();
    // Either path outside the folders makes it an ask that names that path.
    for (paths, outside) in [
        (["inbox/a.txt", "elsewhere/a.txt"], "elsewhere/a.txt"),
        (["../secret", "archive/a.txt"], "../secret"),
        (["inbox/../../x", "archive/a.txt"], "inbox/../../x"),
    ] {
        let verdict = goal.propose(T0, step("fs_move", &paths), Presence::Here, &DOOR);
        let Ok(Verdict::Ask(ask)) = verdict else {
            panic!("{paths:?} should ask: {verdict:?}");
        };
        assert_eq!(ask.kind, AskKind::Scope);
        assert_eq!(
            ask.why,
            format!("{outside} is outside the folders you approved")
        );
        goal.deny().unwrap();
    }
    assert_eq!(
        goal.progress_line(),
        "Goal 'draft the quarterly report': 1 steps done; next: not planned yet; waiting on: nothing"
    );
}

#[test]
fn the_tier_is_the_doors_and_a_step_carries_none() {
    struct Strict;
    impl Door for Strict {
        fn tier(&self, _row: &str, _targets: &[String]) -> Tier {
            Tier::Ask
        }
    }
    let mut goal = goal();
    let trash = step("fs_trash", &["reports/a.txt"]);
    let Ok(Verdict::Ask(ask)) = goal.propose(T0, trash, Presence::Here, &DOOR) else {
        panic!("a row with no undo asks");
    };
    assert_eq!(ask.kind, AskKind::Tier);
    assert_eq!(ask.proposal.map(|p| p.tier), Some(Tier::Ask));
    goal.deny().unwrap();
    // The same step under a door that asks of everything asks too, and the log
    // keeps the tier the door gave.
    let verdict = goal.propose(T0, run_step("fs_list"), Presence::Here, &Strict);
    assert!(matches!(verdict, Ok(Verdict::Ask(_))));
    assert_eq!(goal.log[1].proposal.tier, Tier::Ask);
    goal.deny().unwrap();
    ran(&mut goal, T0, run_step("fs_list"), 1, 10);
    assert_eq!(goal.log[2].proposal.tier, Tier::Run);
}

#[test]
fn a_question_waits_for_an_answer_and_is_no_step() {
    let mut goal = goal();
    goal.plan(3, "next");
    goal.clarify("Which folder holds the Q1 notes?").unwrap();
    assert_eq!((goal.state, goal.asked), (State::Waiting, 1));
    assert!(goal.log.is_empty());
    assert_eq!(
        goal.waiting_line().as_deref(),
        Some(
            "Goal 'draft the quarterly report' is waiting for you: Which folder holds the Q1 notes?"
        )
    );
    assert!(
        goal.progress_line()
            .ends_with("next: your answer; waiting on: Which folder holds the Q1 notes?")
    );
    // There is no step to say yes or no to, and nothing else is asked meanwhile.
    assert_eq!(goal.approve(T0), Err(Refusal::NotAStep));
    assert_eq!(goal.deny(), Err(Refusal::NotAStep));
    assert_eq!(
        goal.propose(T0, run_step("fs_list"), Presence::Here, &DOOR),
        Err(Refusal::NotActive(State::Waiting))
    );
    assert_eq!(
        goal.clarify("again?"),
        Err(Refusal::NotActive(State::Waiting))
    );
    goal.answered().unwrap();
    assert_eq!((goal.state, goal.waiting.is_none()), (State::Active, true));
    assert_eq!(goal.answered(), Err(Refusal::NothingWaiting));
    // A step that asks is a different wait, and counts as another.
    let trash = step("fs_trash", &["reports/a.txt"]);
    goal.propose(T0, trash, Presence::Here, &DOOR).unwrap();
    assert_eq!(goal.answered(), Err(Refusal::NotAQuestion));
    assert_eq!(goal.asked, 2);
}

#[test]
fn a_question_with_no_words_still_says_the_agent_has_one_and_cancel_closes_it() {
    let mut goal = goal();
    goal.clarify("\n\u{7f}").unwrap();
    assert_eq!(goal.waiting.as_ref().unwrap().why, "??");
    goal.answered().unwrap();
    goal.clarify("").unwrap();
    assert_eq!(
        goal.waiting.as_ref().unwrap().why,
        "the agent has a question"
    );
    goal.cancel(T0).unwrap();
    assert_eq!(
        (goal.state, goal.waiting.is_none()),
        (State::Cancelled, true)
    );
    assert!(goal.log.is_empty());
}

#[test]
fn done_is_the_planners_claim_and_met_is_the_runners_check() {
    let mut goal = goal();
    assert_eq!(goal.record_met(true), Err(Refusal::NotDone(State::Active)));
    goal.complete(T0, "the report is written").unwrap();
    assert_eq!((goal.state, goal.met), (State::Done, None));
    goal.record_met(false).unwrap();
    assert_eq!(goal.met, Some(false));
    goal.record_met(true).unwrap();
    assert_eq!(goal.met, Some(true));
}

// ---- the text form

fn busy_goals() -> Goals {
    let mut goals = Goals::new();
    let done_id = goals.set(spec(), T0).unwrap();
    let waiting_id = goals.set(spec(), T0).unwrap();
    let running_id = goals.set(spec(), T0).unwrap();
    let paused_id = goals.set(spec(), T0).unwrap();
    let asking_id = goals.set(spec(), T0).unwrap();
    let _proposed = goals.set(spec(), T0).unwrap();
    for id in [done_id, waiting_id, running_id, paused_id] {
        goals.approve(id, T0 + 1).unwrap();
    }
    let goal = goals.get_mut(done_id).unwrap();
    goal.plan(2, "all done");
    ran(
        goal,
        T0 + 2,
        step("fs_create", &["reports/a b.txt"]),
        5,
        300,
    );
    goal.charge(25);
    goal.complete(T0 + 3, "wrote it").unwrap();
    goal.record_met(true).unwrap();
    let goal = goals.get_mut(waiting_id).unwrap();
    goal.plan(3, "trash the old one");
    ran(goal, T0 + 2, run_step("fs_list"), 6, 10);
    let p = step("fs_trash", &["reports/old.txt"]);
    goal.propose(T0 + 4, p, Presence::Away, &DOOR).unwrap();
    let goal = goals.get_mut(running_id).unwrap();
    goal.propose(T0 + 5, run_step("notes_add"), Presence::Here, &DOOR)
        .unwrap();
    let goal = goals.get_mut(paused_id).unwrap();
    let denied = step("service_restart", &[]);
    goal.propose(T0 + 6, denied, Presence::Here, &DOOR).unwrap();
    goal.deny().unwrap();
    goal.pause().unwrap();
    goals.approve(asking_id, T0 + 1).unwrap();
    let goal = goals.get_mut(asking_id).unwrap();
    goal.plan(2, "after the answer");
    goal.clarify("Which folder holds the Q1 notes?").unwrap();
    goals.tick(T0 + 2 * HOUR);
    goals
}

#[test]
fn goals_read_back_as_they_were_written_byte_for_byte() {
    let goals = busy_goals();
    let text = goals.to_text();
    let back = Goals::parse(&text).unwrap();
    assert_eq!(back, goals);
    assert_eq!(back.to_text(), text);
    assert!(text.starts_with("kai-goals 1\nids\t7\ngoal\t1\tdone\n"));
    // A question that waits is a `waiting` line with no step, and a goal the
    // runner checked has its `met`.
    assert!(text.contains("\nwaiting\tquestion\tWhich folder holds the Q1 notes?\n"));
    assert!(text.contains("\nmet\t1\n"));
}

#[test]
fn an_empty_file_of_goals_round_trips() {
    let text = Goals::new().to_text();
    assert_eq!(text, "kai-goals 1\nids\t1\n");
    assert_eq!(Goals::parse(&text).unwrap(), Goals::new());
}

#[test]
fn the_text_form_has_one_line_a_field_in_a_fixed_order() {
    let mut goals = Goals::new();
    let id = goals.set(spec(), T0).unwrap();
    goals.approve(id, T0).unwrap();
    let text = goals.to_text();
    let expected = "kai-goals 1
ids\t2
goal\t1\tactive
text\tdraft the quarterly report
by\t29800000
away\t1
made\t1000000
started\t1000000
ended\t-
met\t-
note\t
rows\tfs_list\tfs_read\tfs_create\tfs_trash\tnotes_*
folders\treports
budget\t4\t10000\t36000000
checkins\t8200000\t15400000
plan\t0\t
spent\t0\t0\t0\t0\t0\t0
posted\t0\t0\t0
end
";
    assert_eq!(text, expected);
}
