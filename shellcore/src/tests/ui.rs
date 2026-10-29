//! P3 task 10's window rows through the one door: each asks `desktop` through
//! `Host::ui`, `ui_view` takes a second round with the view as data, a call
//! made after reading it is steered (ADR 0002 D6), and `undo` puts window
//! changes back.

use super::act::with_calls;
use super::*;
use crate::ui::{UiCall, Window, parse_windows, state_of};
use alloc::boxed::Box;
use alloc::format;
use alloc::vec;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

const VIEW: &str = "windows: [w2] notes (focused)  [w1] terminal \"kai>\"\n\
                    w2: [t1] textbox \"Search\"  [b2] button \"New note\"";

/// A desktop with Notes focused over the terminal.
fn desktop(host: &FakeHost) {
    *host.ui_answer.borrow_mut() = Some(Box::new(|call: &UiCall| match call {
        UiCall::Windows => Ok(String::from("w2\tnotes\tNotes\t1\nw1\tterminal\tkai>\t0")),
        UiCall::View(_) => Ok(String::from(VIEW)),
        UiCall::Focus(_) => Ok(String::from("w1")),
        UiCall::Close(window) => Ok(window.clone()),
        UiCall::Open { .. } => Ok(String::from("opened w5")),
        // Only the view's ids are known: `b2` and `t1`.
        UiCall::Check { node, .. } if node != "b2" && node != "t1" => {
            Err(String::from("unknown node; view w2 first"))
        }
        _ => Ok(String::new()),
    }));
}

#[test]
fn focus_runs_when_asked_and_undo_puts_it_back() {
    let (host, mut shell) = with_calls(vec![call(
        r#"{"name":"ui_focus","arguments":{"window":"notes"}}"#,
    )]);
    desktop(&host);
    let reply = shell.eval("switch to notes");
    assert_eq!(reply, ok("Done: keyboard focus on notes (was on w1). undo"));
    assert_eq!(*host.ui_calls.borrow(), [UiCall::Focus("notes".into())]);
    assert!(shell.eval("undo").text.starts_with("Undone: "));
    assert_eq!(host.ui_calls.borrow()[1], UiCall::Refocus("w1".into()));
}

#[test]
fn a_view_goes_back_to_the_model_and_its_act_asks() {
    let (host, mut shell) = with_calls(vec![
        call(r#"{"name":"ui_view","arguments":{"window":"notes"}}"#),
        call(
            r#"{"name":"ui_act","arguments":{"window":"notes","node":"b2","action":"click","value":null}}"#,
        ),
    ]);
    desktop(&host);
    let reply = shell.eval("click New note");
    // The second request carried the view as its read.
    let reads = host.act_reads.borrow().clone();
    assert_eq!(reads.len(), 2);
    assert!(reads[0].is_none());
    let read = reads[1].clone().unwrap();
    assert!(read.call.contains("ui_view"));
    assert_eq!(read.data, VIEW);
    // An app's control has no undo: it asks, on the menu.
    let shown = "ui_act(\"notes\", \"b2\", \"click\")";
    assert!(reply.to_wire().starts_with("menu 1 "), "{reply:?}");
    assert!(reply.to_wire().ends_with(shown), "{reply:?}");
    assert!(
        !host
            .ui_calls
            .borrow()
            .iter()
            .any(|c| matches!(c, UiCall::Act { .. }))
    );
    let once = host.mint(1, shown);
    let done = shell.eval(&format!("answer 1 allow once={once}"));
    assert!(done.ok, "{done:?}");
    assert!(host.ui_calls.borrow().contains(&UiCall::Act {
        window: "notes".into(),
        node: "b2".into(),
        action: 0,
        value: String::new(),
    }));
}

#[test]
fn a_change_decided_after_a_view_asks_even_when_the_line_asks_for_it() {
    // The view could say anything: what comes after it is data's choice.
    let (host, mut shell) = with_calls(vec![
        call(r#"{"name":"ui_view","arguments":{"window":"notes"}}"#),
        call(r#"{"name":"set_policy","arguments":{"name":"round-robin"}}"#),
    ]);
    desktop(&host);
    let reply = shell.eval("look at notes and switch to round robin");
    assert!(reply.to_wire().starts_with("menu 1 "), "{reply:?}");
    assert_eq!(host.policy.get(), None);
}

#[test]
fn a_second_view_is_refused_and_a_view_with_no_desktop_says_so() {
    let view = r#"{"name":"ui_view","arguments":{"window":"notes"}}"#;
    let (host, mut shell) = with_calls(vec![call(view), call(view)]);
    desktop(&host);
    let reply = shell.eval("look at notes");
    assert!(
        !reply.ok && reply.text.contains("looked at the window once"),
        "{reply:?}"
    );
    let (_, mut bare) = with_calls(vec![call(view)]);
    let reply = bare.eval("look at notes");
    assert!(!reply.ok && reply.text.contains("no desktop"), "{reply:?}");
}

#[test]
fn close_asks_and_undo_reopens_and_open_runs_and_undo_closes() {
    let close = r#"{"name":"window_close","arguments":{"window":"notes"}}"#;
    let open = r#"{"name":"ui_open","arguments":{"app":"notes","arg":null}}"#;
    let (host, mut shell) = with_calls(vec![call(close), call(open)]);
    desktop(&host);
    let shown = "window_close(\"notes\")";
    let reply = shell.eval("close notes");
    assert!(reply.to_wire().ends_with(shown), "{reply:?}");
    let once = host.mint(1, shown);
    let done = shell.eval(&format!("answer 1 allow once={once}"));
    assert_eq!(done, ok("Done: window notes closed (was open). undo"));
    assert!(shell.eval("undo").ok);
    assert_eq!(
        host.ui_calls.borrow().last(),
        Some(&UiCall::Open {
            app: "notes".into(),
            arg: None
        })
    );
    let opened = shell.eval("open notes");
    assert_eq!(opened, ok("Done: window notes open (was closed). undo"));
    // The window may hold work by now: undo asks before it closes it, and
    // closes that window, not the app's others.
    let asked = shell.eval("undo");
    assert!(asked.text.ends_with("anyway? y/n"), "{asked:?}");
    assert_ne!(
        host.ui_calls.borrow().last(),
        Some(&UiCall::Close("w5".into()))
    );
    assert!(shell.eval("y").ok);
    assert_eq!(
        host.ui_calls.borrow().last(),
        Some(&UiCall::Close("w5".into()))
    );
}

#[test]
fn opening_an_app_already_open_only_focuses_it_and_undo_never_closes_it() {
    let open = r#"{"name":"ui_open","arguments":{"app":"notes"}}"#;
    let (host, mut shell) = with_calls(vec![call(open)]);
    desktop(&host);
    *host.ui_answer.borrow_mut() = Some(Box::new(|call: &UiCall| match call {
        UiCall::Open { .. } => Ok(String::from("focused w2 w1")),
        _ => Ok(String::new()),
    }));
    let reply = shell.eval("open notes");
    assert_eq!(reply, ok("Done: keyboard focus on w2 (was on w1). undo"));
    assert!(shell.eval("undo").ok);
    let calls = host.ui_calls.borrow();
    assert_eq!(calls.last(), Some(&UiCall::Refocus("w1".into())));
    assert!(!calls.iter().any(|c| matches!(c, UiCall::Close(_))));
}

#[test]
fn a_script_or_an_undo_after_a_view_is_refused() {
    let view = r#"{"name":"ui_view","arguments":{"window":"notes"}}"#;
    for after in [
        r#"{"name":"script","arguments":{}}"#,
        r#"{"name":"undo","arguments":{"steps":1}}"#,
    ] {
        let (host, mut shell) = with_calls(vec![call(view), call(after)]);
        desktop(&host);
        let reply = shell.eval("look at notes and fix it");
        assert!(
            !reply.ok && reply.text.contains("act only in it"),
            "{after}: {reply:?}"
        );
        assert!(host.script_requests.borrow().is_empty());
    }
}

#[test]
fn the_window_list_reads_back_into_the_state_line() {
    let windows = parse_windows("w2\tnotes\tShopping\t1\nw1\tfiles\tDocuments\t0").unwrap();
    assert_eq!(
        windows[0],
        Window {
            id: "w2".into(),
            app: "notes".into(),
            title: "Shopping".into(),
            focused: true
        }
    );
    let state = state_of(&windows, crate::state::State::default());
    assert_eq!(
        state.snapshot().unwrap(),
        "#{app: \"notes\", window: \"Shopping\", windows: [\"notes\", \"files\"]}"
    );
    for bad in ["x2\tnotes\tN\t1", "w2\tnotes\tN", "w2\tnotes\tN\tyes"] {
        assert!(parse_windows(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_guessed_node_is_refused_back_to_the_model_which_views_then_acts() {
    let (host, mut shell) = with_calls(vec![
        call(
            r#"{"name":"ui_act","arguments":{"window":"notes","node":"new-note","action":"click"}}"#,
        ),
        call(r#"{"name":"ui_view","arguments":{"window":"w2"}}"#),
        call(r#"{"name":"ui_act","arguments":{"window":"w2","node":"b2","action":"click"}}"#),
    ]);
    desktop(&host);
    let reply = shell.eval("click New note");
    let reads = host.act_reads.borrow().clone();
    assert_eq!(reads.len(), 3);
    // Round two read desktop's refusal, round three the view.
    let refusal = reads[1].clone().unwrap();
    assert!(refusal.call.contains("new-note"));
    assert_eq!(refusal.data, "unknown node; view w2 first");
    assert_eq!(reads[2].clone().unwrap().data, VIEW);
    // The guessed id never reached the user; the viewed one asks.
    let shown = "ui_act(\"w2\", \"b2\", \"click\")";
    assert!(reply.to_wire().ends_with(shown), "{reply:?}");
    assert!(
        !host
            .ui_calls
            .borrow()
            .iter()
            .any(|c| matches!(c, UiCall::Act { .. }))
    );
}

#[test]
fn a_node_still_unknown_after_two_reads_is_refused_with_desktops_words() {
    let guess = |node: &str| {
        call(&format!(
            r#"{{"name":"ui_act","arguments":{{"window":"notes","node":"{node}","action":"click"}}}}"#
        ))
    };
    let view = call(r#"{"name":"ui_view","arguments":{"window":"notes"}}"#);
    let (host, mut shell) = with_calls(vec![guess("a"), view, guess("b")]);
    desktop(&host);
    let reply = shell.eval("click New note");
    assert_eq!(reply, Reply::err("unknown node; view w2 first"));
    assert_eq!(host.act_reads.borrow().len(), 3, "no fourth round");
    // A known node goes to consent at once, with no extra round.
    let (host, mut shell) = with_calls(vec![guess("b2")]);
    desktop(&host);
    shell.eval("click New note");
    assert_eq!(host.act_reads.borrow().len(), 1);
}
