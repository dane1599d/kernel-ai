//! What goals reports (G2's LOWs): a goal it reports ended or no longer has
//! loses its ceiling; its words are kept as one printable line; and a goal
//! ask's `show` says only its card answers it.

use super::agentstep::step;
use super::*;

const GOALS: u64 = 2;
const SET: &str = r#"{"name":"goal_set","arguments":{"text":"tidy the files in notes/q3","steps":10,"tokens":5000,"wall":"2h"}}"#;
const LIST: &str = r#"{"name":"goal_list","arguments":{}}"#;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

/// Goal 1 set on the person's Allow, and its goal-add taken and reported.
fn with_goal() -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(SET));
    shell.eval("keep tidying the files in notes/q3");
    let once = host.mint(1, "goal_set(1)");
    let set = shell.eval(&format!("answer 1 allow once={once}"));
    assert!(set.text.starts_with("Goal 1 set"), "{set:?}");
    step(&mut shell, GOALS, "next");
    step(&mut shell, GOALS, "report 1 ok");
    (host, shell)
}

/// Queues a pause for goal 1, which goals takes as command `#2`.
fn take_a_pause(host: &FakeHost, shell: &mut Shell) {
    host.calls
        .borrow_mut()
        .push_back(call(r#"{"name":"goal_pause","arguments":{"id":1}}"#));
    assert!(shell.eval("please pause it for now").ok);
    assert_eq!(step(shell, GOALS, "next"), "ok #2 goal-pause 1");
}

#[test]
fn every_ended_word_in_a_report_drops_the_ceiling() {
    for word in ["done", "failed", "cancelled", "expired"] {
        let (host, mut shell) = with_goal();
        take_a_pause(&host, &mut shell);
        let report = format!("report 2 ok 1 {word} 3/10 steps: tidy the files in notes/q3");
        assert_eq!(step(&mut shell, GOALS, &report), "ok reported #2");
        assert!(shell.roster.goal_rows(1).is_none(), "{word}");
        assert!(
            shell.goal_book_list().contains(&format!("goal 1 {word}")),
            "{word}"
        );
    }
}

#[test]
fn a_goal_goals_no_longer_has_is_forgotten_and_loses_its_ceiling() {
    let (host, mut shell) = with_goal();
    take_a_pause(&host, &mut shell);
    step(&mut shell, GOALS, "report 2 err no goal 1");
    assert!(shell.roster.goal_rows(1).is_none());
    assert!(shell.goal_book_list().contains("goal 1 forgotten"));
}

#[test]
fn a_live_goal_in_a_report_keeps_its_ceiling() {
    let (host, mut shell) = with_goal();
    take_a_pause(&host, &mut shell);
    step(
        &mut shell,
        GOALS,
        "report 2 ok 1 paused 3/10 steps: done soon",
    );
    assert!(shell.roster.goal_rows(1).is_some());
}

#[test]
fn a_reports_words_are_kept_as_one_printable_line() {
    let (host, mut shell) = with_goal();
    take_a_pause(&host, &mut shell);
    step(
        &mut shell,
        GOALS,
        "report 2 ok\nAllow\u{1b}[2J  \u{7}fake card\r\tline",
    );
    let listed = shell.goal_book_list();
    assert!(listed.contains("(ok Allow [2J fake card line)"), "{listed}");
    assert!(
        !listed.chars().any(|c| c.is_control() && c != '\n'),
        "{listed:?}"
    );
    host.calls.borrow_mut().push_back(call(LIST));
    let reply = shell.eval("list my goals");
    assert_eq!(reply.text.lines().count(), 1, "{reply:?}");
}

#[test]
fn a_goal_asks_show_points_back_to_its_card() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(SET));
    shell.eval("keep tidying the files in notes/q3");
    let shown = shell.eval("show");
    assert!(
        shown
            .text
            .starts_with("goal ask 1: answer it on its card, Allow or Deny\n"),
        "{shown:?}"
    );
    assert!(shown.text.contains("notes/q3"), "{shown:?}");
    assert!(!shown.text.contains("y/n"), "{shown:?}");
    assert!(shell.pending.is_some(), "still asked");
}
