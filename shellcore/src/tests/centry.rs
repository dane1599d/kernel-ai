//! A `c>` entry's `kai` calls (P4 task 12): typed values back, one log entry
//! for the whole entry, content making later changes ask, and asks answered
//! at the prompt.

use super::*;
use serde_json::{Value, json};

fn call(json: &str) -> Call {
    llmproto::act::parse(json).unwrap()
}

const RR: &str = r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#;
const C_RR: &str = "set_policy(\"round-robin\");";

#[test]
fn a_read_comes_back_as_its_typed_value() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    shell.entry_begin("I64 n; cpus(&n); memory(&m);");
    let cpus = shell.entry_call(&call(r#"{"name":"cpus","arguments":{}}"#));
    assert!(
        matches!(cpus.value, Some(Ok(Value::Number(_)))),
        "{:?}",
        cpus.value
    );
    let memory = shell.entry_call(&call(r#"{"name":"memory","arguments":{}}"#));
    let Some(Ok(Value::Object(fields))) = memory.value else {
        panic!("{:?}", memory.value);
    };
    assert!(fields.contains_key("free_mib") && fields.contains_key("heap_kib"));
    assert_eq!(shell.entry_end(true), None);
}

#[test]
fn an_entrys_changes_are_one_log_entry_and_one_undo() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    shell.entry_begin(&format!("{C_RR} fs_create(\"a.txt\", \"one\");"));
    let rr = shell.entry_call(&call(RR));
    assert_eq!(rr.value, Some(Ok(Value::Null)), "{:?}", rr.reply);
    let create = call(r#"{"name":"fs_create","arguments":{"path":"a.txt","text":"one"}}"#);
    let made = shell.entry_call(&create);
    assert_eq!(made.value, Some(Ok(Value::Null)), "{:?}", made.reply);
    let id = shell.entry_end(true).expect("an entry that changed things");
    let entry = shell.log.get(id).unwrap();
    assert_eq!(entry.changes.len(), 2);
    assert_eq!(entry.consent, "c-entry");
    assert!(shell.eval("undo").text.starts_with("Undone: "));
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
    assert!(host.files.borrow().is_empty());
}

#[test]
fn a_failed_entry_is_put_back_at_once() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    shell.entry_begin(C_RR);
    shell.entry_call(&call(RR));
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    shell.entry_end(false);
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn after_content_a_change_asks_and_its_answer_gives_the_value() {
    let host = FakeHost::new();
    host.files
        .borrow_mut()
        .insert("plan.txt".into(), "round-robin".into());
    let mut shell = shell(&host);
    shell.entry_begin(&format!("fs_read(\"plan.txt\", buf, 64); {C_RR}"));
    let read = call(r#"{"name":"fs_read","arguments":{"path":"plan.txt"}}"#);
    assert_eq!(
        shell.entry_call(&read).value,
        Some(Ok(json!("round-robin")))
    );
    let rr = shell.entry_call(&call(RR));
    let asked = rr.reply.confirm.expect("a change after content asks");
    assert_eq!(rr.value, None);
    assert_eq!(host.policy.get(), None);
    let once = host.mint(asked, "set_policy(\"round-robin\")");
    assert!(shell.eval(&format!("answer {asked} allow once={once}")).ok);
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    assert_eq!(shell.take_value(asked), Some(Ok(Value::Null)));
    assert_eq!(shell.take_value(asked), None);
}

#[test]
fn a_declined_ask_and_a_refusal_answer_refused() {
    let host = FakeHost::new();
    *host.procs.borrow_mut() = alloc::vec![String::from("notes")];
    let mut shell = shell(&host);
    shell.entry_begin("process_stop(\"notes\");");
    let stop = shell.entry_call(&call(
        r#"{"name":"process_stop","arguments":{"name":"notes"}}"#,
    ));
    let asked = stop.reply.confirm.expect("a stop asks");
    assert!(shell.eval("n").ok);
    assert_eq!(
        shell.take_value(asked),
        Some(Err(String::from("refused: declined")))
    );
    let unknown = shell.entry_call(&call(r#"{"name":"format_disk","arguments":{}}"#));
    assert!(
        matches!(&unknown.value, Some(Err(why)) if why.starts_with("refused: ")),
        "{:?}",
        unknown.value
    );
    shell.entry_end(true);
}

#[test]
fn a_call_with_no_entry_running_is_refused() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let rr = shell.entry_call(&call(RR));
    assert!(matches!(rr.value, Some(Err(_))));
    assert_eq!(host.policy.get(), None);
}
