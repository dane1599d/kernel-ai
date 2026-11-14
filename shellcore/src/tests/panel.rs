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
fn the_models_words_go_to_the_history_never_the_bar() {
    let (_host, mut shell) = with_calls(vec![call(SAY)]);
    let shown = shell.eval_panel("hi");
    // The bar's line is code's: it says where the words are, not what.
    assert_eq!(shown, PanelResult::Done("Kai answered above.".into()));
    assert_eq!(
        shell.take_panel_said().as_deref(),
        Some("Done: removed 3 files. Your data is safe.")
    );
    // Given once.
    assert_eq!(shell.take_panel_said(), None);
}

#[test]
fn a_model_scripts_result_line_stays_in_the_bar_alone() {
    let (_host, mut shell) = with_calls(vec![call(RR)]);
    let shown = shell.eval_panel("switch to round robin");
    assert!(matches!(shown, PanelResult::Done(_)), "{shown:?}");
    // The result line is the bar's; nothing else was said, so the history
    // gets nothing from Kai.
    let said = shell.take_panel_said();
    assert!(
        said.as_deref()
            .is_none_or(|s| !s.contains("policy round-robin")),
        "{said:?}"
    );
}

#[test]
fn the_persons_own_line_gives_the_history_nothing_from_kai() {
    let (_host, mut shell) = with_calls(vec![]);
    let shown = shell.eval_panel("print(\"hi\"); 1 + 1");
    assert!(matches!(shown, PanelResult::Done(_)), "{shown:?}");
    assert_eq!(shell.take_panel_said(), None);
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
    // A line that asks first shows its script in the terminal, not the history.
    assert_eq!(shell.take_panel_said(), None);
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

#[test]
fn a_typed_models_import_is_refused_at_the_panel_and_a_switch_runs_logged() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let shown = shell.eval_panel("models import");
    assert!(matches!(shown, PanelResult::Refused(_)), "{shown:?}");
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

/// #859's review: the panel's question is the panel's, answered by its next
/// line, never discarded or left for the terminal.
#[test]
fn the_panels_question_is_answered_by_its_next_line() {
    let (host, mut shell) = with_calls(vec![
        call(
            r#"{"name":"clarify","arguments":{"question":"Which policy?","options":["interactive","round-robin"]}}"#,
        ),
        call(RR),
    ]);
    let shown = shell.eval_panel("switch the policy");
    assert_eq!(
        shown,
        PanelResult::Done("Kai asked a question above: answer it here".into())
    );
    assert_eq!(
        shell.take_panel_said().as_deref(),
        Some("Which policy? (interactive, round-robin)")
    );
    // The terminal's own line does not answer it.
    assert!(!shell.eval("1 + 1").text.contains("policy"));
    let shown = shell.eval_panel("round robin");
    assert!(
        matches!(&shown, PanelResult::Done(line) if line.contains("round-robin")),
        "{shown:?}"
    );
    assert_eq!(host.act_lines.borrow()[1], "switch the policy round robin");
}

/// #859's review: a printed claim with no change is Kai's words in the
/// history, and the bar says nothing was changed, never "Done.".
#[test]
fn a_claim_that_changed_nothing_is_kais_and_the_bar_says_so() {
    let (_host, mut shell) = with_calls(vec![call(
        r#"{"name":"script","arguments":{"code":"print(\"Firewall change applied.\");"}}"#,
    )]);
    let shown = shell.eval_panel("turn on the firewall");
    assert_eq!(
        shown,
        PanelResult::Done(crate::guard::NOTHING_CHANGED.into())
    );
    let said = shell.take_panel_said();
    assert_eq!(said.as_deref(), Some("Firewall change applied."));
}

/// #859's re-review: code's question can carry a name the model chose, so the
/// bar shows a fixed line and the question goes to the history as a note,
/// never as Kai's words; the panel's next line answers it.
#[test]
fn codes_question_goes_to_the_history_and_the_bar_stays_fixed() {
    let (host, mut shell) = with_calls(vec![
        call(r#"{"name":"process_stop","arguments":{"name":"process_name"}}"#),
        call(r#"{"name":"process_stop","arguments":{"name":"editor"}}"#),
    ]);
    let shown = shell.eval_panel("stop the process");
    assert_eq!(
        shown,
        PanelResult::Done("A question above: answer it here".into())
    );
    assert_eq!(shell.take_panel_said(), None);
    assert_eq!(
        shell.take_panel_note().as_deref(),
        Some("Which name? \"process_name\" is a placeholder, not one you named.")
    );
    shell.eval_panel("editor");
    assert_eq!(host.act_lines.borrow()[1], "stop the process editor");
}

#[test]
fn new_forgets_the_panels_question() {
    let (host, mut shell) = with_calls(vec![
        call(r#"{"name":"clarify","arguments":{"question":"Which policy?"}}"#),
        call(RR),
    ]);
    shell.eval_panel("switch the policy");
    assert!(shell.eval("new").ok);
    shell.eval_panel("round robin");
    assert_eq!(host.act_lines.borrow()[1], "round robin");
}
