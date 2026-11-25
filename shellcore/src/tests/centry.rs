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

#[test]
fn a_row_kai_h_does_not_bind_is_refused_before_the_door() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    shell.entry_begin("tune(); notes_add(\"x\");");
    for name in ["tune", "component_revert", "notes_add"] {
        let json = format!(r#"{{"name":"{name}","arguments":{{}}}}"#);
        let refused = shell.entry_call(&call(&json));
        assert_eq!(
            refused.value,
            Some(Err(format!("refused: {name} is not a kai call")))
        );
    }
    assert_eq!(
        shell.entry_end(true),
        None,
        "nothing ran, so nothing is logged"
    );
}

/// bug-hunt-3 H3-23: an entry of more calls than the log keeps still holds
/// its first change and leaves the log's earlier entries in it: a failed one
/// puts the change back, and one that ends logs it with its undo.
#[test]
fn an_entry_of_more_calls_than_the_log_keeps_holds_its_first_change() {
    let cpus = call(r#"{"name":"cpus","arguments":{}}"#);
    let create = call(r#"{"name":"fs_create","arguments":{"path":"a.txt","text":"one"}}"#);
    for ok in [false, true] {
        let host = FakeHost::new();
        let mut shell = shell(&host);
        shell.entry_begin("fs_create(\"a.txt\", \"one\");");
        shell.entry_call(&create);
        let earlier = shell.entry_end(true).expect("the file was made");
        shell.entry_begin(&format!("{C_RR} for (I64 i = 0; i < 64; i++) cpus(&n);"));
        shell.entry_call(&call(RR));
        for _ in 0..crate::actlog::KEEP {
            shell.entry_call(&cpus);
        }
        let logged = shell.entry_end(ok);
        if ok {
            let id = logged.expect("the entry changed the policy");
            assert_eq!(shell.log.get(id).unwrap().changes.len(), 1);
            assert!(shell.eval("undo").text.starts_with("Undone: "));
        }
        assert_eq!(host.policy.get(), Some(policy::INTERACTIVE), "ok={ok}");
        assert!(shell.log.get(earlier).is_some(), "ok={ok}");
    }
}

/// bug-hunt-3 H3-21's review: while an entry waits on an answer, its calls so
/// far stand in the log as one entry, and what is done to that stand-in
/// stays done. A file change whose commit `fs` refused at the answer's log
/// write is taken out of it, so the entry never offers to undo it.
#[test]
fn a_file_change_that_did_not_land_while_the_entry_waited_stays_out_of_it() {
    let host = FakeHost::new();
    *host.procs.borrow_mut() = alloc::vec![String::from("notes")];
    host.one_action.set(true);
    let mut shell = shell(&host);
    shell.entry_begin("fs_create(\"a.txt\", \"one\"); process_stop(\"notes\");");
    let create = call(r#"{"name":"fs_create","arguments":{"path":"a.txt","text":"one"}}"#);
    let made = shell.entry_call(&create);
    assert_eq!(made.value, Some(Ok(Value::Null)), "{:?}", made.reply);
    let stop = call(r#"{"name":"process_stop","arguments":{"name":"notes"}}"#);
    let asked = shell.entry_call(&stop).reply.confirm.expect("a stop asks");
    host.commit_refused.set(true);
    let once = host.mint(asked, "process_stop(\"notes\")");
    shell.eval(&format!("answer {asked} allow once={once}"));
    let id = shell.entry_end(true).expect("the stop changed things");
    let entry = shell.log.get(id).unwrap();
    assert!(
        entry.changes.iter().all(|c| c.action != "fs_create"),
        "{entry:?}"
    );
    assert!(entry.outcome.contains("did not land"), "{}", entry.outcome);
}
