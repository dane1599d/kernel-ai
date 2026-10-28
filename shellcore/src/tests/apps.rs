//! The agent-to-app route (P4): an installed app's row runs through the one
//! door as a `kai-call 1`, logged as the app's, undone by its `kai-undo 1`, and
//! its effects run with their own tiers.

use super::act::with_calls;
use super::*;
use alloc::vec;

const NOTES: &str = "version 2
kai-actions 1
world app:notes/notes@1.0.0
action notes_add
export app:notes/actions@1.0.0#add
doc Add a note and return its id.
sig func(text: string) -> result<u32, string>
arg text text max=2000 role=payload from-request
facts read-only=no destructive=no idempotent=no open-world=no
cues add note
inverse snapshot
end
action notes_clear
export app:notes/actions@1.0.0#clear
doc Remove every note.
sig func()
facts read-only=no destructive=yes idempotent=yes open-world=no
cues clear
inverse none
end
";

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

fn with_notes(calls: Vec<Result<Call, ActError>>) -> (Rc<FakeHost>, Shell) {
    let (host, shell) = with_calls(calls);
    host.apps.borrow_mut().rows.load("notes", NOTES).unwrap();
    (host, shell)
}

#[test]
fn an_apps_row_runs_as_a_kai_call_and_undo_puts_its_state_back() {
    let add = r#"{"name":"notes_add","arguments":{"text":"buy milk"}}"#;
    let (host, mut shell) = with_notes(vec![call(add)]);
    host.app_replies
        .borrow_mut()
        .extend([String::from("kai-reply 1\nresult ok(1)\nundo 4\n")]);
    let reply = shell.eval("add a note saying buy milk");
    assert!(reply.ok, "{reply:?}");
    let (app, request) = host.app_requests.borrow()[0].clone();
    assert_eq!(app, "notes");
    assert_eq!(
        request,
        "kai-call 1\nfunc app:notes/actions@1.0.0#add\nargs {text: \"buy milk\"}\n"
    );
    host.app_replies
        .borrow_mut()
        .push_back(String::from("kai-reply 1\nundone 4\nundo 5\n"));
    assert_eq!(shell.eval("undo"), ok("Undone: notes as it was"));
    assert_eq!(host.app_requests.borrow()[1].1, "kai-undo 1\nid 4\n");
}

#[test]
fn an_apps_row_with_no_undo_asks_first() {
    let clear = r#"{"name":"notes_clear","arguments":{}}"#;
    let (host, mut shell) = with_notes(vec![call(clear)]);
    let reply = shell.eval("clear my notes");
    assert!(reply.confirm.is_some(), "{reply:?}");
    assert!(host.app_requests.borrow().is_empty());
}

#[test]
fn an_apps_effects_run_through_the_agent_core() {
    let add = r#"{"name":"notes_add","arguments":{"text":"buy milk"}}"#;
    let (host, mut shell) = with_notes(vec![call(add)]);
    host.app_replies.borrow_mut().push_back(String::from(
        "kai-reply 1\nresult ok(1)\neffect 3 call({name: \"set_policy\", args: \"{name: \\\"round-robin\\\"}\"})\nundo 4\n",
    ));
    assert!(shell.eval("add a note saying buy milk").ok);
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
}

#[test]
fn a_row_no_app_declared_is_no_action() {
    let (_host, mut shell) = with_calls(vec![call(
        r#"{"name":"notes_add","arguments":{"text":"x"}}"#,
    )]);
    assert!(!shell.eval("add a note saying x").ok);
}

#[test]
fn the_first_call_to_an_apps_row_loads_its_rows() {
    let add = r#"{"name":"notes_add","arguments":{"text":"buy milk"}}"#;
    let (host, mut shell) = with_calls(vec![call(add)]);
    host.lists_services.set(true);
    host.services
        .borrow_mut()
        .push((String::from("notes-window"), true));
    host.app_replies.borrow_mut().extend([
        String::from(NOTES),
        String::from("kai-reply 1\nresult ok(1)\nundo 4\n"),
    ]);
    assert!(shell.eval("add a note saying buy milk").ok);
    let requests = host.app_requests.borrow();
    // `notes`, not `notes-window`, whose rows start `notes_window_`.
    assert_eq!(
        requests[0],
        (String::from("notes"), String::from("kai-actions 1"))
    );
    assert_eq!(requests[1].0, "notes");
}
