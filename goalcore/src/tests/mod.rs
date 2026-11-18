//! The goal rules, run against a fake clock.

use super::*;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

mod checkins;
mod runs;
mod storm;
mod text;

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
    fn tier(&self, step: &Step) -> Tier {
        match step.row.as_str() {
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
        call: (),
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
