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
        // Desktop draws the undo control, so the line does not say it.
        PanelResult::Done("Done: policy round-robin (was interactive).".into())
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
fn a_question_the_panel_raises_waits_for_the_card_not_a_typed_yes() {
    let (host, mut shell) = with_calls(vec![call(STOP)]);
    *host.procs.borrow_mut() = vec![String::from("notes")];
    let shown = shell.eval_panel("stop notes");
    assert_eq!(
        shown,
        PanelResult::Done("it asks first: answer Run or Don't on the card".into())
    );
    // A typed yes, in the panel or the console, answers nothing.
    shell.eval_panel("yes");
    shell.eval("yes");
    assert_eq!(*host.procs.borrow(), vec![String::from("notes")]);
    // Don't, on the card: nothing runs, and the panel is told.
    let card = shell.eval_console(termcore::menu::TAKE_CARD);
    let menu = termcore::menu::Menu::decode(&card.to_wire()).unwrap();
    let once = host.mint(menu.id, menu.call());
    shell.eval_console(&format!("answer {} deny once={once}", menu.id));
    assert_eq!(*host.procs.borrow(), vec![String::from("notes")]);
    assert!(shell.take_card_result().is_some());
    assert!(!shell.card_waiting());
}

#[test]
fn at_most_four_questions_wait_for_the_card() {
    let calls = (0..5).map(|_| call(STOP)).collect();
    let (host, mut shell) = with_calls(calls);
    *host.procs.borrow_mut() = vec![String::from("notes")];
    for _ in 0..4 {
        let shown = shell.eval_panel("stop notes");
        assert!(matches!(shown, PanelResult::Done(_)), "{shown:?}");
    }
    let fifth = shell.eval_panel("stop notes");
    assert!(
        matches!(&fifth, PanelResult::Refused(why) if why.contains("other questions wait")),
        "{fifth:?}"
    );
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

#[test]
fn a_typed_models_import_is_refused_at_the_panel_and_a_switch_runs_logged() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    // It has no undo, so it asks: on the card, never at the panel.
    let shown = shell.eval_panel("models import");
    assert!(
        matches!(&shown, PanelResult::Done(line) if line.contains("on the card")),
        "{shown:?}"
    );
    assert!(
        !host
            .model_requests
            .borrow()
            .contains(&"models import".into())
    );
    let shown = shell.eval_panel("models use qwen3-0.6b");
    assert!(
        matches!(&shown, PanelResult::Done(line) if line.contains("next restart")),
        "{shown:?}"
    );
    assert!(shell.eval("log actlog").text.contains("consent=typed"));
    assert_eq!(
        shell.eval("undo"),
        ok("Undone: the model before, from the next restart")
    );
}

#[test]
fn a_question_left_past_two_minutes_lapses_before_its_card() {
    let (host, mut shell) = with_calls(vec![call(STOP)]);
    *host.procs.borrow_mut() = vec![String::from("notes")];
    assert!(matches!(
        shell.eval_panel("stop notes"),
        PanelResult::Done(_)
    ));
    host.clock.set(host.clock.get() + 121_000_000_000);
    let card = shell.eval_console(termcore::menu::TAKE_CARD);
    assert_eq!(card.text, "", "{card:?}");
    assert!(!shell.card_waiting());
    assert!(shell.eval("log").text.contains("expired"));
}
