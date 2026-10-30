//! `shell_line` (P4 task 4): the Terminal's lines run in a terminal of the app's
//! own, cannot ask, and come back as one escaped line.

use super::act::with_calls;
use super::*;
use crate::appline::TERMINAL_APP;
use alloc::vec;

const RR: &str = r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#;
const STOP: &str = r#"{"name":"process_stop","arguments":{"name":"notes"}}"#;

fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The effect line the Terminal queues for `text`.
fn effect(ticket: u32, text: &str) -> String {
    let args = format!("{{line: {}}}", quote(text));
    format!(
        "effect {ticket} call({{name: \"shell_line\", args: {}}})",
        quote(&args)
    )
}

/// What the terminal app gets for `text`.
fn typed(shell: &mut Shell, text: &str) -> Result<String, String> {
    let done = shell.run_effects(TERMINAL_APP, &effect(1, text));
    assert_eq!(done.len(), 1);
    done[0].outcome.clone()
}

#[test]
fn a_line_runs_as_a_terminal_line_and_comes_back_as_its_wire_reply() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(typed(&mut shell, "1 + 2"), Ok(String::from("ok 3")));
    assert_eq!(typed(&mut shell, "("), Ok(String::from("more")));
}

#[test]
fn the_apps_open_entry_is_its_own_and_the_consoles_is_untouched() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    // The console opens an entry; the app has its own.
    let opened = shell.eval("let a = [");
    assert!(opened.more);
    assert_eq!(typed(&mut shell, "fn f() {"), Ok(String::from("more")));
    // The console's next line still continues the console's entry.
    let closed = shell.eval("1]");
    assert!(closed.ok && !closed.more, "{closed:?}");
    // And the app's continues the app's. Rhai's functions are the shell's own,
    // shared by every terminal, as its variables are.
    assert_eq!(typed(&mut shell, "7 }"), Ok(String::from("ok ")));
    assert_eq!(typed(&mut shell, "f()"), Ok(String::from("ok 7")));
    assert!(shell.eval("f()").ok);
}

#[test]
fn a_line_that_would_ask_is_refused_and_the_console_is_not_asked() {
    let (host, mut shell) = with_calls(vec![Ok(llmproto::act::parse(STOP).unwrap())]);
    *host.procs.borrow_mut() = vec![String::from("notes")];
    let said = typed(&mut shell, "stop notes").unwrap();
    assert!(said.starts_with("err it asks first"), "{said}");
    assert_eq!(*host.procs.borrow(), vec![String::from("notes")]);
    // Nothing is left waiting for the console to say yes to.
    let later = shell.eval("y");
    assert!(!later.text.contains("script"), "{later:?}");
    assert_eq!(*host.procs.borrow(), vec![String::from("notes")]);
}

#[test]
fn what_runs_at_once_is_logged_and_undone_like_any_line() {
    let (host, mut shell) = with_calls(vec![Ok(llmproto::act::parse(RR).unwrap())]);
    let said = typed(&mut shell, "switch to round robin").unwrap();
    assert!(said.starts_with("ok "), "{said}");
    assert_eq!(host.policy.get(), Some(policy::ROUND_ROBIN));
    let undone = typed(&mut shell, "undo").unwrap();
    assert!(undone.starts_with("ok "), "{undone}");
    assert_eq!(host.policy.get(), Some(policy::INTERACTIVE));
}

#[test]
fn only_the_terminal_may_run_lines_and_never_an_answer() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    for app in ["files", "notes-window", "settings", ""] {
        let done = shell.run_effects(app, &effect(1, "1 + 1"));
        assert!(
            matches!(&done[0].outcome, Err(why) if why.contains("may not run shell lines")),
            "{app}: {done:?}"
        );
    }
    assert_eq!(
        typed(&mut shell, "answer 1 yes"),
        Ok(String::from("err answers go on the question's own prompt"))
    );
}

#[test]
fn bad_arguments_come_back_as_errors() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    for args in [
        "{}",
        "{line: 3}",
        "{line: \\\"a\\\", extra: \\\"b\\\"}",
        "x",
    ] {
        let line = format!("effect 1 call({{name: \"shell_line\", args: \"{args}\"}})");
        let done = shell.run_effects(TERMINAL_APP, &line);
        assert!(done[0].outcome.is_err(), "{args}: {done:?}");
    }
}

#[test]
fn the_answer_is_one_line_escaped_and_cut_at_a_character() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(
        typed(&mut shell, r#""a\nb\\c""#),
        Ok(String::from(r"ok a\nb\\c"))
    );
    let long = typed(&mut shell, r#"let s = ""; for i in 0..300 { s += "é"; } s"#).unwrap();
    assert!(long.len() <= 190 + '…'.len_utf8(), "{} bytes", long.len());
    assert!(long.ends_with('…') && long.starts_with("ok é"), "{long}");
    assert!(!long.contains('\n'));
    // A cut never lands inside an escape: every backslash in what is left pairs.
    let tail = typed(
        &mut shell,
        r#"let s = ""; for i in 0..300 { s += "\n"; } s"#,
    )
    .unwrap();
    assert!(tail.len() <= 190 + '…'.len_utf8(), "{tail}");
    let body = tail.strip_suffix('…').unwrap().strip_prefix("ok ").unwrap();
    assert!(!body.contains('\n') && body.matches('\\').count() == body.matches("\\n").count());
}
