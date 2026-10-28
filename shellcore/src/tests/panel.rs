//! Panel lines (P3 task 7): they keep out of the terminal's entry, question and
//! proposal, and the panel shows code's result lines only.

use super::act::with_calls;
use super::*;
use crate::PanelResult;
use alloc::vec;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

const RR: &str = r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#;
const STOP: &str = r#"{"name":"process_stop","arguments":{"name":"notes"}}"#;
const SAY: &str =
    r#"{"name":"say","arguments":{"text":"Done: removed 3 files. Your data is safe."}}"#;

#[test]
fn a_run_tier_request_shows_codes_line() {
    let (host, mut shell) = with_calls(vec![call(RR)]);
    assert_eq!(
        shell.eval_panel("switch to round robin"),
        PanelResult::Done("Done: policy round-robin (was interactive). undo".into())
    );
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
}

#[test]
fn the_models_words_never_reach_the_panel() {
    let (_host, mut shell) = with_calls(vec![call(SAY)]);
    let shown = shell.eval_panel("hi");
    assert!(
        matches!(&shown, PanelResult::Refused(why) if why.contains("answered in words")),
        "{shown:?}"
    );
}

#[test]
fn a_panel_yes_does_not_answer_the_terminals_question() {
    let (host, mut shell) = with_calls(vec![call(STOP)]);
    *host.procs.borrow_mut() = vec![String::from("notes")];
    let asked = shell.eval("stop notes");
    assert!(asked.confirm.is_some(), "{asked:?}");
    let shown = shell.eval_panel("yes");
    assert!(!matches!(shown, PanelResult::Done(ref line) if line.contains("stop")));
    assert_eq!(*host.procs.borrow(), vec![String::from("notes")]);
    // The terminal's question is still the terminal's: it waits on its menu.
    let answered = shell.eval("y");
    assert!(answered.text.starts_with("script 1 "), "{answered:?}");
    assert_eq!(*host.procs.borrow(), vec![String::from("notes")]);
}

#[test]
fn a_question_the_panel_raises_is_dropped_not_left_for_a_yes() {
    let (host, mut shell) = with_calls(vec![call(STOP)]);
    *host.procs.borrow_mut() = vec![String::from("notes")];
    let shown = shell.eval_panel("stop notes");
    assert!(
        matches!(&shown, PanelResult::Refused(why) if why.contains("asks first")),
        "{shown:?}"
    );
    shell.eval_panel("yes");
    shell.eval("yes");
    assert_eq!(*host.procs.borrow(), vec![String::from("notes")]);
}

#[test]
fn a_panel_line_neither_joins_nor_leaves_an_open_terminal_entry() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert!(shell.eval("let x = [1,").more);
    let shown = shell.eval_panel("let y = (2");
    assert!(matches!(shown, PanelResult::Refused(_)), "{shown:?}");
    assert_eq!(shell.eval("2]; x.len()"), ok("2"));
}

#[test]
fn the_users_own_line_shows_its_first_result_line() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(
        shell.eval_panel("print(\"\"); 40 + 2"),
        PanelResult::Done("42".into())
    );
    assert_eq!(
        shell.eval_panel("answer 1 yes"),
        PanelResult::Refused("answers go in the terminal".into())
    );
}
