//! The shell-side guard against two shapes the shipped-pack actbench found
//! (#715): placeholder arguments, and scripts that only print success.

use super::act::with_calls;
use super::*;
use crate::guard::{NOTHING_CHANGED, claims, placeholder};
use alloc::vec;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

fn script(code: &str) -> Result<Call, ActError> {
    let call = serde_json::json!({"name": "script", "arguments": {"code": code}});
    Ok(llmproto::act::parse(&call.to_string()).unwrap())
}

#[test]
fn placeholders_are_told_from_names() {
    let line = "remove the package";
    for value in [
        "package_name",
        "<package>",
        "{name}",
        "[app]",
        "your_package",
        "service name",
        "name_here",
        "/path/to/file",
        "<path>",
        "your_file",
        "example",
    ] {
        assert!(placeholder(value, line), "{value}");
    }
    for value in [
        "notes",
        "spell-en",
        "weather",
        "netd",
        "round-robin",
        "my-notes2",
        "todo",
    ] {
        assert!(!placeholder(value, line), "{value}");
    }
    // What the user wrote is theirs, whatever it looks like.
    assert!(!placeholder("foo", "remove foo"));
    assert!(!placeholder("example", "Open the website example"));
}

#[test]
fn a_name_written_with_other_joins_is_still_the_users() {
    // The app gate's todo request (#730): "to-do" names `todo`.
    assert!(!placeholder(
        "todo",
        "a to-do list where I can tick items off"
    ));
    assert!(!placeholder("build-4", "stop build 4"));
    assert!(!placeholder("my_notes", "open my-notes"));
    // A join is not a name: placeholders the line doesn't hold still refuse.
    let line = "make me a to-do list app";
    for value in ["package_name", "<path>", "your_file", "app_name"] {
        assert!(placeholder(value, line), "{value}");
    }
}

#[test]
fn claims_are_told_from_plain_output() {
    for text in [
        "Change applied.",
        "Policy has been set",
        "Done!",
        "Successfully removed",
    ] {
        assert!(claims(text), "{text}");
    }
    for text in ["4", "CPUs: 4", "hello"] {
        assert!(!claims(text), "{text}");
    }
}

#[test]
fn a_call_naming_a_placeholder_asks_which_and_runs_nothing() {
    let (host, mut shell) = with_calls(vec![
        call(r#"{"name":"process_stop","arguments":{"name":"process_name"}}"#),
        call(r#"{"name":"process_stop","arguments":{"name":"editor"}}"#),
    ]);
    let before = host.procs.borrow().clone();
    let reply = shell.eval("stop the process");
    assert_eq!(
        reply,
        ok("Which name? \"process_name\" is a placeholder, not one you named.")
    );
    assert_eq!(*host.procs.borrow(), before);
    // The answer joins the request, as a clarify answer does.
    shell.eval("editor");
    assert_eq!(host.act_lines.borrow()[1], "stop the process editor");
}

#[test]
fn a_script_holding_a_placeholder_is_refused() {
    let (host, mut shell) = with_calls(vec![script("fs_create(\"path/to/file.txt\", \"hi\");")]);
    let before = host.procs.borrow().clone();
    let reply = shell.eval("make a file saying hi");
    assert!(!reply.ok, "{reply:?}");
    assert!(
        reply.text.contains("\"path/to/file.txt\" is a placeholder"),
        "{reply:?}"
    );
    assert_eq!(*host.procs.borrow(), before);
    assert!(!shell.eval("ls").text.contains("file.txt"));
}

#[test]
fn a_script_that_only_prints_success_says_nothing_was_changed() {
    let (host, mut shell) = with_calls(vec![
        script("print(\"Change applied.\");"),
        script("print(\"hello\");"),
        script("let x = 1;"),
    ]);
    let policy = host.policy.get();
    assert_eq!(
        shell.eval("switch to round robin"),
        ok(&format!("Change applied.\n{NOTHING_CHANGED}"))
    );
    assert_eq!(host.policy.get(), policy);
    // Output that claims nothing is shown as it is.
    assert_eq!(shell.eval("say hello"), ok("hello\n"));
    // And a script that does nothing at all is not "Done."
    assert_eq!(shell.eval("do the thing"), ok(NOTHING_CHANGED));
}

#[test]
fn a_script_with_os_calls_keeps_its_own_lines() {
    let (_host, mut shell) = with_calls(vec![script("print(\"CPUs: \" + cpus());")]);
    assert_eq!(shell.eval("tell me the cpu count"), ok("CPUs: 4\n"));
}

#[test]
fn a_read_then_a_fake_claim_still_says_nothing_was_changed() {
    let (host, mut shell) = with_calls(vec![script(
        "let n = cpus(); print(\"Policy has been set for \" + n + \" CPUs.\");",
    )]);
    let policy = host.policy.get();
    assert_eq!(
        shell.eval("switch to round robin"),
        ok(&format!(
            "Policy has been set for 4 CPUs.\n{NOTHING_CHANGED}"
        ))
    );
    assert_eq!(host.policy.get(), policy);
}

#[test]
fn a_payload_that_looks_like_a_placeholder_is_written_as_given() {
    // Only targets are checked (C on #716): what goes in a file is the user's.
    for (number, text) in ["[1, 2, 3]", "<p>hi</p>", "TODO"].into_iter().enumerate() {
        let path = format!("note{number}.txt");
        let created = serde_json::json!({
            "name": "fs_create",
            "arguments": {"path": path, "text": text},
        });
        let scripted = format!("fs_write({path:?}, {text:?});");
        let (host, mut shell) = with_calls(vec![
            Ok(llmproto::act::parse(&created.to_string()).unwrap()),
            script(&scripted),
        ]);
        let reply = shell.eval(&format!("create {path}"));
        assert!(reply.text.starts_with("Done: "), "{text}: {reply:?}");
        assert_eq!(
            host.files.borrow().get(&path).map(String::as_str),
            Some(text)
        );
        let reply = shell.eval(&format!("write to {path}"));
        assert!(!reply.text.contains("placeholder"), "{text}: {reply:?}");
    }
}
