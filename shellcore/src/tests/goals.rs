//! The goal rows (G2): `goal_set`'s one ask sets the goal's ceiling and
//! queues it for `goals`, which takes it with `agent-step next` and answers
//! with `agent-step report`; pause and resume undo each other; cancel asks and
//! drops the ceiling; no agent sets or changes a goal.

use super::agentstep::{step, with_agent};
use super::*;
use crate::goalrows::{GOALS_ONLY, MAX_QUEUED, PERSON_ONLY};

/// The `goals` agent's badge.
const GOALS: u64 = 2;
const SET: &str = r#"{"name":"goal_set","arguments":{"text":"tidy the files in notes/q3","steps":10,"tokens":5000,"wall":"2h"}}"#;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

/// A shell with goal 1 set on the person's allow (request 1).
fn with_goal() -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(SET));
    let asked = shell.eval("keep tidying the files in notes/q3 for the next two hours");
    // The card: code-written counts, Deny highlighted, the short call.
    assert_eq!(
        asked.text,
        concat!(
            "menu 1 added
",
            "Goal: 2 actions in 1 folder, up to 10 steps and 5000 tokens, ",
            "with goal_set(1)? Type show to see them


goal_set(1)"
        )
    );
    // `show` gives the whole approval: what it may use and spend.
    let shown = shell.eval("show");
    assert!(
        shown.text.contains("notes/q3") && shown.text.contains("10 steps"),
        "{shown:?}"
    );
    let once = host.mint(1, "goal_set(1)");
    let set = shell.eval(&format!("answer 1 allow once={once}"));
    assert!(set.text.starts_with("Goal 1 set"), "{set:?}");
    (host, shell)
}

fn goals_step(shell: &mut Shell, text: &str) -> String {
    step(shell, GOALS, text)
}

#[test]
fn a_goal_is_set_on_the_persons_allow_with_its_ceiling_and_queued_for_goals() {
    let (_, mut shell) = with_goal();
    let rows = shell
        .roster
        .goal_rows(1)
        .expect("goal 1's ceiling")
        .to_vec();
    assert!(rows.iter().any(|r| r == "fs_list"), "{rows:?}");
    assert!(!rows.iter().any(|r| r.starts_with("goal_")), "{rows:?}");
    let next = goals_step(&mut shell, "next");
    assert!(
        next.starts_with("ok #1 goal-add 1\nkai-goals 1\n"),
        "{next}"
    );
    let file = next.split_once('\n').unwrap().1;
    let goals = goalcore::Goals::parse(file).expect("a kai-goals 1 file");
    let goal = goals.get(1).expect("goal 1");
    assert_eq!(goal.scope.folders, ["notes/q3"]);
    assert!(goal.state.is_live(), "approved: {:?}", goal.state);
    assert_eq!(goals_step(&mut shell, "next"), "ok none");
    assert_eq!(goals_step(&mut shell, "report 1 ok"), "ok reported #1");
    assert!(
        shell.goal_book_list().contains("goal 1 running"),
        "{}",
        shell.goal_book_list()
    );
}

#[test]
fn a_deny_sets_nothing_and_queues_nothing() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(SET));
    shell.eval("keep tidying the files in notes/q3");
    shell.eval("n");
    assert!(shell.roster.goal_rows(1).is_none());
    assert_eq!(goals_step(&mut shell, "next"), "ok none");
}

#[test]
fn a_goal_that_names_nothing_it_does_is_refused_without_an_ask() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(
        r#"{"name":"goal_set","arguments":{"text":"be good","steps":3,"tokens":100,"wall":"1h"}}"#,
    ));
    let reply = shell.eval("be good for an hour");
    assert!(
        !reply.ok && reply.text.contains("say what the goal does"),
        "{reply:?}"
    );
    assert!(shell.pending.is_none());
}

#[test]
fn a_time_not_written_as_minutes_hours_or_days_is_refused() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(
        r#"{"name":"goal_set","arguments":{"text":"tidy notes/q3","steps":3,"tokens":100,"wall":"soon"}}"#,
    ));
    let reply = shell.eval("tidy notes/q3 soon");
    assert!(
        !reply.ok && reply.text.contains("90m, 2h or 1d"),
        "{reply:?}"
    );
}

#[test]
fn pause_and_resume_are_queued_and_undo_each_other() {
    let (host, mut shell) = with_goal();
    goals_step(&mut shell, "next");
    goals_step(&mut shell, "report 1 ok");
    host.calls
        .borrow_mut()
        .push_back(call(r#"{"name":"goal_pause","arguments":{"id":1}}"#));
    let paused = shell.eval("please pause it for now");
    assert!(paused.text.starts_with("Done: goal 1 paused"), "{paused:?}");
    assert_eq!(goals_step(&mut shell, "next"), "ok #2 goal-pause 1");
    let undone = shell.eval_console("undo");
    assert!(undone.ok, "{undone:?}");
    assert_eq!(goals_step(&mut shell, "next"), "ok #3 goal-resume 1");
}

#[test]
fn cancel_asks_and_its_allow_drops_the_ceiling() {
    let (host, mut shell) = with_goal();
    goals_step(&mut shell, "next");
    goals_step(&mut shell, "report 1 ok");
    host.calls
        .borrow_mut()
        .push_back(call(r#"{"name":"goal_cancel","arguments":{"id":1}}"#));
    let asked = shell.eval("cancel goal 1");
    assert!(asked.text.contains("goal_cancel(1)"), "{asked:?}");
    assert!(shell.roster.goal_rows(1).is_some(), "not before the yes");
    let id = asked.confirm.unwrap();
    let once = host.mint(id, "goal_cancel(1)");
    let done = shell.eval(&format!("answer {id} allow once={once}"));
    assert!(done.text.starts_with("Goal 1 cancelled"), "{done:?}");
    assert!(shell.roster.goal_rows(1).is_none());
    assert_eq!(goals_step(&mut shell, "next"), "ok #2 goal-cancel 1");
}

#[test]
fn only_goals_takes_the_queue_and_no_agent_sets_a_goal() {
    let (_, mut shell, badge) = with_agent();
    assert_eq!(step(&mut shell, badge, "next"), format!("err {GOALS_ONLY}"));
    assert_eq!(
        step(&mut shell, badge, "report 1 ok"),
        format!("err {GOALS_ONLY}")
    );
    // goals holds no grant to the goal rows, and the row refuses an agent.
    let set = goals_step(&mut shell, SET);
    assert!(set.starts_with("err refused"), "{set}");
    let _ = PERSON_ONLY;
}

#[test]
fn goals_refusing_a_goal_drops_its_ceiling() {
    let (_, mut shell) = with_goal();
    goals_step(&mut shell, "next");
    goals_step(&mut shell, "report 1 err too many active goals");
    assert!(shell.roster.goal_rows(1).is_none());
    assert!(shell.goal_book_list().contains("goal 1 not started"));
}

#[test]
fn the_queue_holds_at_most_its_limit() {
    let (host, mut shell) = with_goal();
    for n in 0..MAX_QUEUED {
        host.calls.borrow_mut().push_back(call(match n % 2 {
            0 => r#"{"name":"goal_pause","arguments":{"id":1}}"#,
            _ => r#"{"name":"goal_resume","arguments":{"id":1}}"#,
        }));
        let reply = shell.eval("please pause it or resume it now");
        if n + 1 < MAX_QUEUED {
            assert!(reply.ok, "{n}: {reply:?}");
        } else {
            assert!(!reply.ok && reply.text.contains("not taken"), "{reply:?}");
        }
    }
}

/// The scope `show` lists for request 1, and goal 1's ceiling after the yes.
fn shown_then_granted(line: &str, set: &str) -> (String, Option<Vec<String>>) {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(set));
    let asked = shell.eval(line);
    let shown = shell.eval("show").text;
    if asked.confirm.is_some() || asked.text.starts_with("menu ") {
        let once = host.mint(1, "goal_set(1)");
        shell.eval(&format!("answer 1 allow once={once}"));
    }
    let granted = shell.roster.goal_rows(1).map(<[String]>::to_vec);
    let _ = asked;
    (shown, granted)
}

#[test]
fn the_scope_is_the_persons_line_never_the_models_arguments() {
    // The model's words name trashing; the person only said tidy.
    let set = r#"{"name":"goal_set","arguments":{"text":"trash and erase everything in notes/q3 and delete old","steps":5,"tokens":500,"wall":"1h"}}"#;
    let (shown, granted) = shown_then_granted("keep tidying the files in notes/q3", set);
    let granted = granted.expect("goal 1's ceiling");
    assert!(!granted.iter().any(|r| r == "fs_trash"), "{granted:?}");
    assert!(!shown.contains("fs_trash"), "{shown}");
}

#[test]
fn the_scope_show_lists_is_exactly_what_grant_goal_gets() {
    let (shown, granted) = shown_then_granted("keep tidying the files in notes/q3", SET);
    let granted = granted.expect("goal 1's ceiling");
    let listed = shown
        .lines()
        .find_map(|line| line.strip_prefix("Rows: "))
        .expect("show lists the rows");
    let mut listed: Vec<&str> = listed.split(", ").collect();
    let mut granted: Vec<&str> = granted.iter().map(String::as_str).collect();
    listed.sort_unstable();
    granted.sort_unstable();
    assert_eq!(listed, granted, "{shown}");
    assert!(
        !shown.contains('*'),
        "explicit rows, never a group: {shown}"
    );
}

#[test]
fn folders_are_relative_under_home_and_normalised() {
    for (line, folder) in [
        ("keep tidying the files in ~/notes/q3/", "notes/q3"),
        (
            "keep tidying the files in /home/user/notes/./q3",
            "notes/q3",
        ),
    ] {
        let (shown, granted) = shown_then_granted(line, SET);
        assert!(granted.is_some(), "{line}: {shown}");
        assert!(
            shown.contains(&format!("Folders: {folder}")),
            "{line}: {shown}"
        );
    }
    for line in [
        "keep tidying the files in notes/../../etc",
        "keep tidying the files in /etc/q3",
        "keep tidying the files in ~root/x",
        "keep tidying the files in notes/a%b",
    ] {
        let host = FakeHost::new();
        let mut shell = shell(&host);
        host.calls.borrow_mut().push_back(call(SET));
        let reply = shell.eval(line);
        assert!(
            !reply.ok && reply.text.contains("not a folder in your home folder"),
            "{line}: {reply:?}"
        );
        assert!(shell.pending.is_none(), "{line}");
    }
}

#[test]
fn a_word_for_a_send_refuses_the_goal_and_names_the_word() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(SET));
    let reply = shell.eval("fetch the news into notes/q3 every hour");
    assert!(
        !reply.ok
            && reply.text.contains("\"fetch\" names ")
            && reply.text.contains("a goal may not do"),
        "{reply:?}"
    );
    assert!(shell.pending.is_none());
}

#[test]
fn more_than_eight_actions_is_refused() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(SET));
    let reply = shell.eval(
        "read, create, write, move, trash, restore and interactive history of the files in notes/q3",
    );
    assert_eq!(
        reply,
        Reply::err(format!("no goal set: {}", crate::goalscope::SAY_LESS))
    );
}

#[test]
fn a_folders_path_names_no_action() {
    // "notes" in the path is no cue for fs_create.
    let (shown, granted) = shown_then_granted("keep tidying the files in notes/q3", SET);
    let granted = granted.expect("goal 1's ceiling");
    assert!(
        !granted.iter().any(|r| r == "fs_create"),
        "{granted:?}: {shown}"
    );
}

#[test]
fn a_negated_action_refuses_the_goal_and_names_the_word() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(SET));
    let reply = shell.eval("keep tidying the files in notes/q3 but never move them");
    assert!(
        !reply.ok && reply.text.contains("\"move\" is said not to happen"),
        "{reply:?}"
    );
    assert!(shell.pending.is_none());
}

#[test]
fn a_pause_the_line_did_not_ask_for_asks_on_the_card() {
    let (host, mut shell) = with_goal();
    goals_step(&mut shell, "next");
    goals_step(&mut shell, "report 1 ok");
    host.calls
        .borrow_mut()
        .push_back(call(r#"{"name":"goal_pause","arguments":{"id":1}}"#));
    let asked = shell.eval("keep the tidying going");
    assert!(
        asked.text.starts_with(
            "menu 2 added
Pause goal 1 with goal_pause(1)? "
        ),
        "{asked:?}"
    );
    assert_eq!(
        goals_step(&mut shell, "next"),
        "ok none",
        "nothing queued before the yes"
    );
    let once = host.mint(2, "goal_pause(1)");
    let done = shell.eval(&format!("answer 2 allow once={once}"));
    assert!(done.text.starts_with("Done: goal 1 paused"), "{done:?}");
    assert_eq!(shell.log.get(2).unwrap().consent, "yes");
    assert_eq!(goals_step(&mut shell, "next"), "ok #2 goal-pause 1");
}

#[test]
fn an_approved_cancel_drops_the_ceiling_even_with_the_queue_full() {
    let (host, mut shell) = with_goal();
    // goals took the add and has not reported it: it holds its place.
    goals_step(&mut shell, "next");
    for n in 1..MAX_QUEUED {
        host.calls.borrow_mut().push_back(call(match n % 2 {
            1 => r#"{"name":"goal_pause","arguments":{"id":1}}"#,
            _ => r#"{"name":"goal_resume","arguments":{"id":1}}"#,
        }));
        assert!(shell.eval("please pause it or resume it now").ok, "{n}");
    }
    host.calls
        .borrow_mut()
        .push_back(call(r#"{"name":"goal_cancel","arguments":{"id":1}}"#));
    let asked = shell.eval("cancel goal 1");
    let id = asked.confirm.expect("asked");
    let once = host.mint(id, "goal_cancel(1)");
    let done = shell.eval(&format!("answer {id} allow once={once}"));
    assert!(done.text.starts_with("Goal 1 cancelled"), "{done:?}");
    assert!(shell.roster.goal_rows(1).is_none());
    // Its untaken commands went; the cancel is queued.
    let mut taken = Vec::new();
    loop {
        let next = goals_step(&mut shell, "next");
        if next == "ok none" {
            break;
        }
        taken.push(next);
    }
    assert!(
        taken.last().unwrap().ends_with("goal-cancel 1"),
        "{taken:?}"
    );
    assert!(!taken.iter().any(|t| t.contains("goal-pause")), "{taken:?}");
}

#[test]
fn undo_puts_back_the_state_before_and_never_revives_a_cancelled_goal() {
    let (host, mut shell) = with_goal();
    for _ in 0..2 {
        host.calls
            .borrow_mut()
            .push_back(call(r#"{"name":"goal_pause","arguments":{"id":1}}"#));
        shell.eval("please pause it for now");
    }
    // The second pause found it paused: its undo leaves it paused.
    let last = shell.log.last_id();
    let inverse = shell.log.get(last).unwrap().changes[0].inverse;
    assert_eq!(
        inverse,
        Some(crate::undo::Inverse::Goal {
            id: 1,
            paused: true
        })
    );
    host.calls
        .borrow_mut()
        .push_back(call(r#"{"name":"goal_cancel","arguments":{"id":1}}"#));
    let asked = shell.eval("cancel goal 1");
    let id = asked.confirm.expect("asked");
    let once = host.mint(id, "goal_cancel(1)");
    shell.eval(&format!("answer {id} allow once={once}"));
    assert_eq!(
        shell.revert_goal(1, false),
        Err(String::from("goal 1 is cancelled"))
    );
}

#[test]
fn goal_ids_are_never_used_twice_after_a_restart() {
    let (host, mut shell) = with_goal();
    // A restart: the book is gone, the log is kept.
    shell.goal_book = crate::goalrows::GoalBook::default();
    host.calls.borrow_mut().push_back(call(SET));
    let asked = shell.eval("keep tidying the files in notes/q3");
    let id = asked.confirm.expect("asked");
    let once = host.mint(id, &format!("goal_set({id})"));
    let set = shell.eval(&format!("answer {id} allow once={once}"));
    assert!(set.text.starts_with("Goal 2 set"), "{set:?}");
}

/// H2-7: the shell stamps a goal in the boot's ms, and the goals agent runs it
/// in the same ms (`goalcore::now_ms`). Run against wall-clock ms instead, as
/// the agent did, the goal was over its wall budget at its first turn and no
/// step ever ran.
#[test]
fn a_goal_the_shell_sets_runs_its_first_step_on_the_agents_clock() {
    use goalcore::runner::{Event, Outcome, Parts, Plan, Planner, Runner, Steps};
    use goalcore::{Door, Goal, Presence, Step, StepDone, Tier};
    struct OneStep;
    impl Planner<()> for OneStep {
        fn next(&mut self, _goal: &Goal, _now: u64) -> (Plan<()>, u64) {
            let step = Step {
                row: String::from("fs_list"),
                targets: alloc::vec![String::from("notes/q3")],
                tokens_est: 0,
                call: (),
            };
            (Plan::Step(step), 0)
        }
    }
    struct InCeiling;
    impl Door<()> for InCeiling {
        fn tier(&self, _step: &Step<()>) -> Tier {
            Tier::Run
        }
    }
    struct Ran;
    impl Steps<()> for Ran {
        fn run(&mut self, _goal: u64, _step: &Step<()>) -> Outcome {
            Outcome::Done(StepDone {
                entry: Some(9),
                tokens: 0,
                failed: None,
            })
        }
    }
    let turn = |file: &str, now: u64| {
        let mut goals = goalcore::Goals::parse(file).unwrap();
        let mut parts = Parts {
            planner: &mut OneStep,
            door: &InCeiling,
            steps: &mut Ran,
        };
        Runner::new().turn(&mut goals, now, Presence::Here, &mut parts)
    };
    let (host, mut shell) = with_goal();
    // Ten minutes after boot, on a machine whose firmware gave the date.
    host.clock.set(600_000_000_000);
    host.wall_clock.set(Some(1_790_000_000));
    let next = goals_step(&mut shell, "next");
    let file = next.split_once('\n').unwrap().1;
    let events = turn(file, goalcore::now_ms(host.clock.get()));
    assert!(
        matches!(events.as_slice(), [Event::Ran { goal: 1, .. }]),
        "{events:?}"
    );
    // The agent's old clock, wall-clock ms: paused before any step.
    let events = turn(file, 1_790_000_000 * 1000);
    assert!(
        events.iter().any(|e| matches!(e, Event::Paused { .. })),
        "{events:?}"
    );
}

/// bug-hunt-3 H3-24: a shell started again reads its goals from the log on
/// disk, `goal_1` for `goal 1`, and never gives goal id 1 again.
#[test]
fn goal_ids_are_never_used_twice_after_a_shell_restart() {
    let (host, _first) = with_goal();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(SET));
    let asked = shell.eval("keep tidying the files in notes/q3");
    let id = asked.confirm.expect("asked");
    let once = host.mint(id, &format!("goal_set({id})"));
    let set = shell.eval(&format!("answer {id} allow once={once}"));
    assert!(set.text.starts_with("Goal 2 set"), "{set:?}");
}

/// #16: a `by` that fits a u64 but not one added to now is refused, never
/// an overflow (an abort in a debug build).
#[test]
fn a_deadline_too_far_off_to_add_to_now_is_refused() {
    let host = FakeHost::new();
    host.clock.set(60_000_000_000);
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(
        r#"{"name":"goal_set","arguments":{"text":"tidy notes/q3","steps":3,"tokens":100,"wall":"1h","by":"307445734561825m"}}"#,
    ));
    let reply = shell.eval("keep tidying the files in notes/q3 until much later");
    assert!(
        !reply.ok && reply.text.contains("90m, 2h or 1d"),
        "{reply:?}"
    );
    assert!(shell.pending.is_none());
}

/// #18: a goal step's path the folder check calls inside is inside where
/// the fs rows act too: `~/` is home for both.
#[test]
fn a_path_inside_a_goals_folders_is_inside_them_where_the_files_are() {
    let folders = [String::from("notes")];
    for path in ["~/notes/x", "~/notes", "/home/user/notes/x", "notes/x", "~"] {
        if !crate::goalscope::path_inside(&folders, path) {
            continue;
        }
        let at = crate::script::home_path(path);
        assert!(
            at.is_some_and(|at| at == "notes" || at.starts_with("notes/")),
            "{path} passes the check but is {at:?} to the fs rows"
        );
    }
    assert_eq!(crate::script::home_path("~"), Some(""));
}

/// Sets one more goal on the person's allow and has `goals` take it: the
/// reply to the line, and to the allow when there was a card.
fn set_another(host: &FakeHost, shell: &mut Shell) -> (Reply, Option<Reply>) {
    host.calls.borrow_mut().push_back(call(SET));
    let asked = shell.eval("keep tidying the files in notes/q3 for the next two hours");
    let Some(menu) = asked
        .text
        .strip_prefix("menu ")
        .and_then(|rest| rest.split_once(' '))
        .and_then(|(n, _)| n.parse::<u64>().ok())
    else {
        return (asked, None);
    };
    let short = asked.text.lines().last().unwrap().to_string();
    let once = host.mint(menu, &short);
    let set = shell.eval(&format!("answer {menu} allow once={once}"));
    let next = goals_step(shell, "next");
    let n = next
        .strip_prefix("ok #")
        .unwrap()
        .split_once(' ')
        .unwrap()
        .0;
    goals_step(shell, &format!("report {n} ok"));
    (asked, Some(set))
}

/// bug-hunt-6 H6-25: with sixteen goals running, a seventeenth is refused
/// before the card, never after the person's yes; once one ends, the next is
/// asked and set.
#[test]
fn a_seventeenth_goal_is_refused_before_the_ask_and_set_once_one_ends() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    for n in 1..=goalcore::MAX_GOALS {
        let (_, set) = set_another(&host, &mut shell);
        let set = set.expect("a card");
        assert!(set.text.starts_with(&format!("Goal {n} set")), "{set:?}");
    }
    let (asked, set) = set_another(&host, &mut shell);
    assert!(set.is_none(), "no card for the seventeenth: {asked:?}");
    assert!(
        !asked.ok && asked.text.contains("16 goals are running or paused"),
        "{asked:?}"
    );
    assert!(shell.roster.goal_rows(17).is_none());
    // `goals` ends goal 1: its place comes back.
    goals_step(&mut shell, "goal=1 end");
    let (_, set) = set_another(&host, &mut shell);
    let set = set.expect("a card once a goal ended");
    assert!(set.text.starts_with("Goal 17 set"), "{set:?}");
}

/// A joined folder name is the name's words (#1467 review): its "delete"
/// names no row and its "not" negates none, so "sort the files in my
/// do-not-delete folder" is a goal that lists files, with no trash or remove
/// in it and no refusal.
#[test]
fn a_cue_inside_a_folder_name_adds_no_row() {
    let scope = crate::goalscope::scope("sort the files in my do-not-delete folder", |_| Ok(()));
    assert_eq!(
        scope.map(|scope| scope.rows),
        Ok(alloc::vec!["fs_list".to_string()])
    );
    // A delete said outside a name is still read, and its negation too.
    let scope = crate::goalscope::scope("sort notes/q3 and never delete anything", |_| Ok(()));
    assert!(scope.is_err(), "{scope:?}");
    // A joined word of only negation is no name: it still says not to
    // (#1467's round 20).
    for line in [
        "sort notes/q3 and do-not move anything",
        "sort notes/q3 and don't-ever move anything",
    ] {
        let scope = crate::goalscope::scope(line, |_| Ok(()));
        assert!(scope.is_err(), "{line}: {scope:?}");
    }
}

/// A dropped name leaves its punctuation (#1467's round 21): "todo.txt," still
/// ends the "don't" clause, so the move after it is asked for.
#[test]
fn a_dropped_name_keeps_its_clause_break() {
    let scope = crate::goalscope::scope(
        "sort notes/q3: don't touch todo.txt, move the rest into notes/old",
        |_| Ok(()),
    );
    assert!(
        scope
            .as_ref()
            .is_ok_and(|scope| scope.rows.iter().any(|r| r == "fs_move")),
        "{scope:?}"
    );
}

/// A negation joined to the cue is no name (#1467's round 21): "dont-delete
/// the finals" still says not to, unless a determiner or a possessive makes
/// it a folder's name.
#[test]
fn a_negation_joined_to_the_cue_still_refuses_a_goal() {
    for line in [
        "trash the drafts in notes/q3, dont-delete the finals",
        "sort notes/q3, never-move the finals",
        "sort notes/q3 and do-not-move anything",
        // A folder's path between a determiner and the word is no name's
        // lead (#1556's review).
        "trash the drafts in my notes/q3, dont-delete the finals",
        "trash the drafts in my notes/q3 dont-delete the finals",
    ] {
        let scope = crate::goalscope::scope(line, |_| Ok(()));
        assert!(scope.is_err(), "{line}: {scope:?}");
    }
    for line in [
        "sort the files in my do-not-delete folder",
        "sort the files in Bob's dont-delete folder",
    ] {
        let scope = crate::goalscope::scope(line, |_| Ok(()));
        assert_eq!(
            scope.map(|scope| scope.rows),
            Ok(alloc::vec!["fs_list".to_string()]),
            "{line}"
        );
    }
}

/// #1577's review: a word that ends its clause leads nothing, so "this:"
/// before "dont-delete" leaves it a negation; and a folder said not to be
/// touched is never a goal's folder, in either order.
#[test]
fn a_clause_mark_ends_a_lead_and_a_folder_kept_off_refuses_the_goal() {
    for line in [
        "trash the drafts in notes/q3, keep this: dont-delete the finals",
        "don't touch notes/q3, trash the drafts in notes/old",
        "trash the drafts in notes/old, don't touch notes/q3",
        "sort everything in notes/ except notes/q3",
    ] {
        let scope = crate::goalscope::scope(line, |_| Ok(()));
        assert!(scope.is_err(), "{line}: {scope:?}");
    }
    let kept_off = crate::goalscope::scope(
        "don't touch notes/q3, trash the drafts in notes/old",
        |_| Ok(()),
    );
    assert_eq!(
        kept_off,
        Err(String::from(
            "\"notes/q3\" is said not to be touched; name only the folders the goal works in"
        ))
    );
    // A negation after the folder, or describing files, keeps it.
    for line in [
        "sort notes/q3, it's not urgent",
        "move the files with no tag into notes/untagged",
    ] {
        let scope = crate::goalscope::scope(line, |_| Ok(()));
        assert!(scope.is_ok(), "{line}: {scope:?}");
    }
}

/// A quoted negation still refuses the goal (#1467 review): its cue is read
/// even inside the quotes.
#[test]
fn a_quoted_negation_still_refuses_a_goal() {
    let scope = crate::goalscope::scope(
        "sort the files in notes/ by topic, \"never move anything\"",
        |_| Ok(()),
    );
    assert!(scope.is_err(), "{scope:?}");
}
