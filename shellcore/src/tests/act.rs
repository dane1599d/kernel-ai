//! The one door: tagged calls from `act`, with consent, tiers and the result line
//! decided in code.

use super::*;
use alloc::vec;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

fn with_calls(calls: Vec<Result<Call, ActError>>) -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    host.calls.borrow_mut().extend(calls);
    let shell = shell(&host);
    (host, shell)
}

const RR: &str = r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#;
const INTERACTIVE: &str = r#"{"name":"set_policy","arguments":{"name":"interactive"}}"#;

#[test]
fn a_plain_undoable_request_runs_at_once_with_a_code_written_line() {
    let (host, mut shell) = with_calls(vec![call(RR)]);
    let reply = shell.eval("switch to round robin");
    assert_eq!(
        reply,
        ok("Done: policy round-robin (was interactive). undo")
    );
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    assert_eq!(*host.act_lines.borrow(), ["switch to round robin"]);
    // No script was asked for: the call was enough.
    assert!(host.script_requests.borrow().is_empty());
    assert!(shell.eval("undo").text.starts_with("Undone: "));
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn a_read_answers_with_its_value() {
    let (_host, mut shell) = with_calls(vec![call(r#"{"name":"cpus","arguments":{}}"#)]);
    assert_eq!(shell.eval("how many cpus are there"), ok("4"));
}

#[test]
fn a_vague_line_runs_its_one_undoable_reading_and_says_what_it_took() {
    let (host, mut shell) = with_calls(vec![call(INTERACTIVE)]);
    host.policy.set(Some(policy::ROUND_ROBIN));
    let reply = shell.eval("make it snappier");
    assert_eq!(
        reply,
        ok("Taken as: policy interactive (was round-robin). undo")
    );
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn a_line_that_is_not_english_asks_and_runs_only_on_a_yes() {
    let (host, mut shell) = with_calls(vec![call(RR), call(RR)]);
    let reply = shell.eval("cambia la política a round robin");
    assert_eq!(reply.confirm, Some(1), "{reply:?}");
    assert!(reply.text.contains("not in English"), "{reply:?}");
    assert_eq!(host.policy.get(), None);
    assert_eq!(shell.eval("n").text, "script 1 not run");
    let reply = shell.eval("cambia la política a round robin");
    assert_eq!(reply.confirm, Some(2));
    assert!(shell.eval("y").text.starts_with("Done: policy round-robin"));
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
}

#[test]
fn a_negated_or_added_change_asks() {
    let (host, mut shell) = with_calls(vec![call(RR)]);
    let reply = shell.eval("show the policy, but don't switch to round robin");
    assert_eq!(reply.confirm, Some(1), "{reply:?}");
    assert!(reply.text.contains("did not ask for it"), "{reply:?}");
    assert_eq!(host.policy.get(), None);
}

#[test]
fn never_values_and_rows_the_shell_cannot_call_are_refused() {
    let (_host, mut shell) = with_calls(vec![
        call(r#"{"name":"tune","arguments":{"target":"shell"}}"#),
        call(r#"{"name":"component_revert","arguments":{"component":"notes"}}"#),
        call(r#"{"name":"set_policy","arguments":{"name":"fifo"}}"#),
        call(r#"{"name":"rm_rf","arguments":{}}"#),
    ]);
    let never = shell.eval("please tune the shell process");
    assert!(!never.ok && never.text.contains("never run"), "{never:?}");
    let typed = shell.eval("revert notes");
    assert!(
        !typed.ok && typed.text.contains("typed at the prompt"),
        "{typed:?}"
    );
    let bad = shell.eval("switch the policy to fifo");
    assert!(!bad.ok && bad.text.contains("not a valid name"), "{bad:?}");
    let unknown = shell.eval("delete everything please");
    assert!(
        !unknown.ok && unknown.text.contains("no such action"),
        "{unknown:?}"
    );
}

#[test]
fn say_answers_in_words_and_clarify_joins_the_reply_to_the_request() {
    let (host, mut shell) = with_calls(vec![
        call(r#"{"name":"say","arguments":{"text":"Hello there."}}"#),
        call(
            r#"{"name":"clarify","arguments":{"question":"Which policy?","options":["interactive","round-robin"]}}"#,
        ),
        call(RR),
    ]);
    assert_eq!(shell.eval("hello"), ok("Hello there."));
    assert_eq!(
        shell.eval("switch the policy"),
        ok("Which policy? (interactive, round-robin)")
    );
    let reply = shell.eval("round robin");
    assert!(
        reply.text.starts_with("Done: policy round-robin"),
        "{reply:?}"
    );
    assert_eq!(host.act_lines.borrow()[2], "switch the policy round robin");
}

#[test]
fn a_script_call_runs_through_todays_script_checks() {
    let (host, mut shell) = with_calls(vec![
        call(r#"{"name":"script","arguments":{"code":"print(\"CPUs: \" + cpus());"}}"#),
        call(r#"{"name":"script","arguments":{"code":"while true {}"}}"#),
    ]);
    assert_eq!(
        shell.eval("tell me the cpu count"),
        ok("CPUs: 4
")
    );
    let refused = shell.eval("loop forever");
    assert!(!refused.ok, "{refused:?}");
    assert!(host.script_requests.borrow().is_empty());
    // `show` prints the code, with no model summary.
    let shown = shell.eval("show").text;
    assert!(!shown.contains("model says"), "{shown}");
}

#[test]
fn with_no_call_the_shell_asks_for_a_script_as_before() {
    let host = FakeHost::new();
    host.answers.borrow_mut().push_back(Ok(Answer {
        script: String::from("print(cpus());"),
        summary: String::from("Prints the CPU count."),
    }));
    let mut shell = shell(&host);
    assert_eq!(
        shell.eval("how many cpus are there"),
        ok("4
")
    );
    assert_eq!(host.act_lines.borrow().len(), 1);
    assert_eq!(host.script_requests.borrow().len(), 1);
}
