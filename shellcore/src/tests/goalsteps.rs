//! A goal's steps (G2): `agent-step goal=<id> <call>` runs under that goal's
//! ceiling only, never `goals`' own grants; `end` drops the ceiling; a wait
//! for a step never asked is `err unknown step`.

use super::agentstep::{step, with_agent};
use super::*;
use crate::goalrows::GOALS_ONLY;

const GOALS: u64 = 2;
const SET: &str = r#"{"name":"goal_set","arguments":{"text":"tidy the files in notes/q3","steps":10,"tokens":5000,"wall":"2h"}}"#;
const LIST_Q3: &str = r#"{"name":"fs_list","arguments":{"path":"notes/q3"}}"#;
const RR: &str = r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

/// Goal 1 set on the person's Allow, its add taken and reported.
fn with_goal() -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(SET));
    shell.eval("keep tidying the files in notes/q3");
    let once = host.mint(1, "goal_set(1)");
    assert!(shell.eval(&format!("answer 1 allow once={once}")).ok);
    step(&mut shell, GOALS, "next");
    step(&mut shell, GOALS, "report 1 ok");
    (host, shell)
}

#[test]
fn a_step_inside_the_goals_ceiling_runs_under_its_goal() {
    let (_, mut shell) = with_goal();
    let wire = step(&mut shell, GOALS, &format!("goal=1 {LIST_Q3}"));
    assert!(wire.starts_with("ok run #"), "{wire}");
    let id = shell.log.last_id();
    assert_eq!(shell.log.get(id).unwrap().agent.as_str(), "goals");
}

#[test]
fn a_row_outside_the_ceiling_or_no_goal_is_refused() {
    let (host, mut shell) = with_goal();
    let outside = step(&mut shell, GOALS, &format!("goal=1 {RR}"));
    assert!(outside.starts_with("err refused"), "{outside}");
    // goals holds no grants of its own: a step with no goal is refused.
    let bare = step(&mut shell, GOALS, LIST_Q3);
    assert!(bare.starts_with("err refused"), "{bare}");
    // Another goal's id holds no ceiling.
    let other = step(&mut shell, GOALS, &format!("goal=7 {LIST_Q3}"));
    assert!(other.starts_with("err refused"), "{other}");
    assert_eq!(host.policy.get(), None);
}

#[test]
fn end_drops_the_ceiling_and_later_steps_are_refused() {
    let (_, mut shell) = with_goal();
    assert_eq!(step(&mut shell, GOALS, "goal=1 end"), "ok ended goal 1");
    assert!(shell.roster.goal_rows(1).is_none());
    assert!(shell.goal_book_list().contains("goal 1 ended"));
    let after = step(&mut shell, GOALS, &format!("goal=1 {LIST_Q3}"));
    assert!(after.starts_with("err refused"), "{after}");
}

#[test]
fn only_goals_ends_a_goal() {
    let (_, mut shell, badge) = with_agent();
    assert_eq!(
        step(&mut shell, badge, "goal=1 end"),
        format!("err {GOALS_ONLY}")
    );
}

#[test]
fn a_wait_for_a_step_never_asked_is_unknown() {
    let (_, mut shell) = with_goal();
    assert_eq!(step(&mut shell, GOALS, "wait 99"), "err unknown step #99");
}

fn list(path: &str) -> String {
    format!(r#"goal=1 {{"name":"fs_list","arguments":{{"path":"{path}"}}}}"#)
}

#[test]
fn a_goal_step_acts_only_inside_the_folders_the_person_approved() {
    let (_, mut shell) = with_goal();
    assert!(step(&mut shell, GOALS, &list("notes/q3/sub")).starts_with("ok run #"));
    assert!(step(&mut shell, GOALS, &list("/home/user/notes/q3")).starts_with("ok run #"));
    for outside in [
        "notes/q4",
        "notes/q3x/plan.txt",
        "notes/q3/../../etc",
        "/etc/passwd",
        "~root/notes/q3",
        "",
    ] {
        let wire = step(&mut shell, GOALS, &list(outside));
        assert!(
            wire.starts_with("err refused") && wire.contains("outside the folders"),
            "{outside}: {wire}"
        );
    }
}

/// H6-27: net_fetch's `path` is a URL's, not a home-folder path, so a goal that
/// may fetch a page is not refused for the page's path.
#[test]
fn a_net_fetch_path_is_a_url_path_not_a_home_folder_one() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(
        r#"{"name":"goal_set","arguments":{"text":"save the web page from example.com in notes/q3","steps":10,"tokens":5000,"wall":"2h"}}"#,
    ));
    shell.eval("save the web page from example.com in notes/q3 and keep at it");
    let once = host.mint(1, "goal_set(1)");
    assert!(shell.eval(&format!("answer 1 allow once={once}")).ok);
    step(&mut shell, GOALS, "next");
    step(&mut shell, GOALS, "report 1 ok");
    assert!(
        shell
            .roster
            .goal_rows(1)
            .unwrap()
            .iter()
            .any(|r| r == "net_fetch"),
        "{:?}",
        shell.roster.goal_rows(1)
    );
    for path in ["/forecast", "/index.html", "forecast?day=1", ""] {
        let fetch = format!(
            r#"goal=1 {{"name":"net_fetch","arguments":{{"host":"example.com","path":"{path}"}}}}"#
        );
        let wire = step(&mut shell, GOALS, &fetch);
        assert!(!wire.contains("outside the folders"), "{path:?}: {wire}");
    }
    // A file row's path is still held to the folders.
    let wire = step(&mut shell, GOALS, &list("/forecast"));
    assert!(wire.contains("outside the folders"), "{wire}");
}

#[test]
fn a_readmitted_row_stays_under_its_goal() {
    let (_, mut shell) = with_goal();
    let goals = shell.roster.caller(2).unwrap();
    let admitted = shell
        .roster
        .admit_for_goal_in(&goals, 1, "fs_list", &agentabi::ROWS)
        .unwrap();
    shell.acting = Some(admitted);
    // A row the goal's ceiling holds: re-admitted under the same goal.
    assert!(shell.readmit(&agentabi::FS_READ).is_ok());
    assert_eq!(shell.acting.as_ref().and_then(|a| a.goal()), Some(1));
    // One it does not hold is refused, whatever goals' own grants say.
    assert!(shell.readmit(&agentabi::FS_TRASH).is_err());
    shell.acting = None;
}

#[test]
fn a_yes_given_after_the_goal_ended_runs_nothing() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(
        r#"{"name":"goal_set","arguments":{"text":"move the files in notes/q3 into folders","steps":10,"tokens":5000,"wall":"2h"}}"#,
    ));
    shell.eval("move the files in notes/q3 into folders and keep at it");
    let once = host.mint(1, "goal_set(1)");
    assert!(shell.eval(&format!("answer 1 allow once={once}")).ok);
    step(&mut shell, GOALS, "next");
    step(&mut shell, GOALS, "report 1 ok");
    assert!(
        shell
            .roster
            .goal_rows(1)
            .unwrap()
            .iter()
            .any(|r| r == "fs_move"),
        "{:?}",
        shell.roster.goal_rows(1)
    );
    host.files
        .borrow_mut()
        .insert(String::from("notes/q3/a.txt"), String::from("a"));
    let moved =
        r#"goal=1 {"name":"fs_move","arguments":{"from":"notes/q3/a.txt","to":"notes/q3/b.txt"}}"#;
    let asked = step(&mut shell, GOALS, moved);
    assert!(asked.starts_with("ok asked "), "{asked}");
    // The goal ends while its step waits on the card.
    assert_eq!(step(&mut shell, GOALS, "goal=1 end"), "ok ended goal 1");
    let card = shell.eval_console(termcore::menu::TAKE_CARD);
    let menu = termcore::menu::Menu::decode(&card.to_wire()).expect("the step's card");
    let once = host.mint(menu.id, menu.call());
    let answered = shell.eval_console(&format!("answer {} allow once={once}", menu.id));
    assert!(!answered.ok, "{answered:?}");
    assert!(
        host.files.borrow().contains_key("notes/q3/a.txt"),
        "nothing moved"
    );
    let told = step(&mut shell, GOALS, &format!("wait {}", menu.id));
    assert!(
        told.starts_with(&format!("err not run #{}", menu.id)),
        "{told}"
    );
}

#[test]
fn a_move_with_either_end_outside_the_folders_is_refused() {
    let (_, mut shell) = with_goal();
    for (from, to) in [
        ("notes/q3/a.txt", "notes/q4/a.txt"),
        ("notes/q4/a.txt", "notes/q3/a.txt"),
        ("notes/q3/a.txt", "notes/q3/../a.txt"),
    ] {
        let moved =
            format!(r#"goal=1 {{"name":"fs_move","arguments":{{"from":"{from}","to":"{to}"}}}}"#);
        let wire = step(&mut shell, GOALS, &moved);
        assert!(
            wire.starts_with("err refused") && wire.contains("outside the folders"),
            "{from} -> {to}: {wire}"
        );
    }
}

#[test]
fn rows_that_reach_past_the_folders_are_never_a_goals() {
    // Named in the person's line, they refuse the goal, and say which word.
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(SET));
    let reply = shell.eval("keep tidying and find the files in notes/q3");
    assert!(
        !reply.ok && reply.text.contains("\"find\" names fs_find"),
        "{reply:?}"
    );
    // And a step to one is refused even under a goal.
    let (_, mut shell) = with_goal();
    for json in [
        r#"{"name":"fs_find","arguments":{"name":"plan"}}"#,
        r#"{"name":"fs_changes","arguments":{}}"#,
        r#"{"name":"ui_windows","arguments":{}}"#,
    ] {
        let wire = step(&mut shell, GOALS, &format!("goal=1 {json}"));
        assert!(wire.starts_with("err refused"), "{json}: {wire}");
    }
}
