//! The action log: `undo N`, the record each model turn leaves for llm's memory,
//! `new`, and `log actlog`.

use super::*;
use alloc::vec;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

const RR: &str = r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#;
const INTERACTIVE: &str = r#"{"name":"set_policy","arguments":{"name":"interactive"}}"#;
const CPUS: &str = r#"{"name":"cpus","arguments":{}}"#;

fn with_calls(calls: Vec<Result<Call, ActError>>) -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    host.calls.borrow_mut().extend(calls);
    let shell = shell(&host);
    (host, shell)
}

#[test]
fn undo_n_refuses_to_overwrite_a_later_change_and_names_it() {
    let (host, mut shell) = with_calls(vec![call(RR), call(CPUS), call(INTERACTIVE)]);
    shell.eval("switch to round robin");
    shell.eval("how many cpus are there");
    shell.eval("set the policy to interactive");
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
    let refused = shell.eval("undo 1");
    assert_eq!(
        refused,
        Reply::err("#3 changed the same thing since #1; undo it first")
    );
    assert_eq!(shell.eval("undo 2"), Reply::err("#2 has nothing to undo"));
    assert_eq!(shell.eval("undo 9"), Reply::err("no logged entry #9"));
    assert_eq!(
        shell.eval("undo 3"),
        ok("Undone: policy back to round-robin")
    );
    assert_eq!(
        shell.eval("undo 1"),
        ok("Undone: policy back to interactive")
    );
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
    let list = shell.eval("log").text;
    assert!(list.contains("#4 undo of #3: done"), "{list}");
    assert!(list.contains("#5 undo of #1: done"), "{list}");
}

#[test]
fn each_model_turn_is_remembered_once_with_its_result_line() {
    let (host, mut shell) = with_calls(vec![call(RR), call(RR)]);
    shell.eval("switch to round robin");
    // A turn that asks is remembered when it is answered.
    let asked = shell.eval("cambia la política a round robin");
    assert!(asked.confirm.is_some());
    assert_eq!(host.remembered.borrow().len(), 1);
    shell.eval("n");
    // Typed lines and a typed undo are not model turns.
    shell.eval("undo");
    shell.eval("1 + 1");
    let remembered = host.remembered.borrow();
    assert_eq!(remembered.len(), 2);
    assert_eq!(
        remembered[0],
        (
            String::from("switch to round robin"),
            String::from(r#"{"arguments":{"name":"round-robin"},"name":"set_policy"}"#),
            String::from("Done: policy round-robin (was interactive). undo"),
        )
    );
    assert_eq!(remembered[1].2, "Not run: declined");
}

#[test]
fn new_forgets_the_conversation_but_not_the_log() {
    let (host, mut shell) = with_calls(vec![call(RR)]);
    shell.eval("switch to round robin");
    let reply = shell.eval("new");
    assert!(reply.text.starts_with("A new conversation"), "{reply:?}");
    assert_eq!(host.forgot.get(), 1);
    assert_eq!(shell.eval("undo"), ok("Undone: policy back to interactive"));
}

#[test]
fn log_actlog_prints_the_records() {
    let (_host, mut shell) = with_calls(vec![call(RR)]);
    shell.eval("switch to round robin");
    shell.eval("undo");
    let text = shell.eval("log actlog").text;
    let records = crate::actlog::parse(&text).unwrap_or_else(|e| panic!("{e}: {text:?}"));
    assert_eq!(records.len(), 2, "{text}");
    assert!(
        text.contains("form=call tier=run consent=asked state=undone"),
        "{text}"
    );
    assert!(text.contains("\nU 2 of=1 "), "{text}");
}
