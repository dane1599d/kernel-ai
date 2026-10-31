//! `appearance_set` (P4 task 8): a plainly asked setting runs at once, is
//! saved, and `undo` puts the old value back; `look` refuses what would break
//! the theme, and a value the line did not give asks.

use super::act::with_calls;
use super::*;
use alloc::vec;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

fn theme(host: &FakeHost) -> String {
    crate::appearance::current(host).theme
}

#[test]
fn the_rows_settings_are_looks() {
    assert_eq!(agentabi::APPEARANCE_SETTINGS, ::look::appearance::SETTINGS);
}

#[test]
fn a_plain_theme_switch_runs_saves_and_undoes() {
    let light = r#"{"name":"appearance_set","arguments":{"setting":"theme","value":"light"}}"#;
    let (host, mut shell) = with_calls(vec![call(light)]);
    let reply = shell.eval("switch to the light theme");
    assert!(reply.ok && reply.text.ends_with(". undo"), "{reply:?}");
    // Each word once: the setting, its value, and what it was.
    assert_eq!(reply.text.matches("theme").count(), 1, "{reply:?}");
    assert_eq!(reply.text.matches("(was dark)").count(), 1, "{reply:?}");
    assert_eq!(theme(&host), "light");
    assert!(
        host.appearance
            .borrow()
            .as_deref()
            .unwrap()
            .starts_with("kai-appearance 1\n")
    );
    assert!(shell.eval("undo").text.starts_with("Undone: "));
    assert_eq!(theme(&host), "dark");
}

#[test]
fn a_theme_that_does_not_exist_is_refused_and_nothing_changes() {
    let bad = r#"{"name":"appearance_set","arguments":{"setting":"theme","value":"neon"}}"#;
    let (host, mut shell) = with_calls(vec![call(bad)]);
    let reply = shell.eval("switch to the neon theme");
    assert!(!reply.ok, "{reply:?}");
    assert!(host.appearance.borrow().is_none());
}

#[test]
fn a_value_the_line_did_not_give_asks_first() {
    let size = r#"{"name":"appearance_set","arguments":{"setting":"text_size","value":"200"}}"#;
    let (host, mut shell) = with_calls(vec![call(size)]);
    let reply = shell.eval("make the text bigger");
    assert!(reply.confirm.is_some(), "{reply:?}");
    assert!(host.appearance.borrow().is_none());
}

#[test]
fn undo_leaves_a_setting_changed_since_alone() {
    let light = r#"{"name":"appearance_set","arguments":{"setting":"theme","value":"light"}}"#;
    let (host, mut shell) = with_calls(vec![call(light)]);
    assert!(shell.eval("switch to the light theme").ok);
    // Settings, or another change, picked high-contrast since.
    let changed = crate::appearance::current(host.as_ref())
        .set("theme", "high-contrast", &::look::appearance::Shipped)
        .unwrap();
    *host.appearance.borrow_mut() = Some(changed.encode());
    let reply = shell.eval("undo");
    assert!(!reply.text.starts_with("Undone: "), "{reply:?}");
    assert_eq!(theme(&host), "high-contrast");
}
