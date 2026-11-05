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
        "TODO",
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
    ] {
        assert!(!placeholder(value, line), "{value}");
    }
    // What the user wrote is theirs, whatever it looks like.
    assert!(!placeholder("foo", "remove foo"));
    assert!(!placeholder("example", "Open the website example"));
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
