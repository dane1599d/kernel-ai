//! Whole runs on the fake clock, the label-reading door, and goals of more than one path.

use super::*;

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

// ---- a door that reads the call's labels (ADR 0002 D6)

/// The runner's record of a call: where its text came from, if read content.
#[derive(Clone, Copy)]
struct Call {
    text_from: Option<&'static str>,
}

/// A door that asks for a step content steers, though its row runs.
struct Labels;

impl Door<Call> for Labels {
    fn tier(&self, step: &Step<Call>) -> Tier {
        match step.call.text_from {
            Some(_) => Tier::Ask,
            None => Tier::Run,
        }
    }

    fn reason(&self, step: &Step<Call>) -> Option<String> {
        step.call
            .text_from
            .map(|source| format!("the text came from {source}"))
    }
}

fn write(text_from: Option<&'static str>) -> Step<Call> {
    Step {
        row: String::from("fs_write"),
        targets: strings(&["reports/q1.txt"]),
        tokens_est: 100,
        call: Call { text_from },
    }
}

#[test]
fn a_door_that_reads_the_calls_labels_asks_for_a_step_content_steers_even_when_away() {
    let mut goal = goal();
    goal.scope.rows.push(String::from("fs_write"));
    // The row alone runs: a write the person's own words asked for.
    assert_eq!(
        goal.propose(T0, write(None), Presence::Away, &Labels),
        Ok(Verdict::Run)
    );
    goal.finish_step(done(1, 10)).unwrap();
    // The same row and the same path, with text read from a note: it asks, the
    // person away or not, and says where the text came from.
    let Ok(Verdict::Ask(ask)) =
        goal.propose(T0, write(Some("note \"todo\"")), Presence::Away, &Labels)
    else {
        panic!("a steered step should ask");
    };
    assert_eq!(ask.kind, AskKind::Tier);
    assert_eq!(ask.why, "the text came from note \"todo\"");
    assert_eq!(goal.state, State::Waiting);
    assert_eq!(goal.log[1].proposal.tier, Tier::Ask);
    assert_eq!(
        goal.waiting_line().as_deref(),
        Some(
            "Goal 'draft the quarterly report' is waiting for you: the text came from note \"todo\""
        )
    );
    // Nothing ran: only the unsteered write did.
    assert_eq!((goal.steps_run, goal.flight), (1, None));
    // A door that says nothing about why leaves the row's own words.
    goal.deny().unwrap();
    let verdict = goal.propose(T0, run_step("fs_trash"), Presence::Here, &DOOR);
    assert!(
        matches!(verdict, Ok(Verdict::Ask(ref a)) if a.why == "fs_trash has no undo, so it asks")
    );
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
        fn tier(&self, _step: &Step) -> Tier {
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
