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

#[test]
fn a_save_from_before_shell_and_hover_keeps_its_settings_and_audits_the_colour() {
    // Written by a build that had no `hover`: a light theme with a `muted` that is
    // 4.32:1 on it, and the person's scale and legible type.
    let saved =
        "kai-appearance 1\ntheme = light\ntoken = muted #666666\nscale = 150\nlegible = on\n";
    let (host, _shell) = with_calls(vec![]);
    *host.appearance.borrow_mut() = Some(String::from(saved));
    let now = crate::appearance::current(host.as_ref());
    assert_eq!(
        (now.theme.as_str(), now.scale, now.legible),
        ("light", 150, true)
    );
    assert!(now.tokens.is_empty());
    let audit = host.audit.borrow().join("\n");
    assert!(
        audit.contains("the saved colour muted #666666 is dropped: muted on hover is 4.3"),
        "{audit}"
    );
    // A save that was short before is still refused whole, and says so.
    let short = "kai-appearance 1\ntheme = dark\ntoken = muted #30343C\nscale = 150\n";
    *host.appearance.borrow_mut() = Some(String::from(short));
    let now = crate::appearance::current(host.as_ref());
    assert_eq!((now.theme.as_str(), now.scale), ("dark", 100));
    assert!(
        host.audit
            .borrow()
            .iter()
            .any(|l| l.starts_with("the saved appearance is refused: muted on window"))
    );
}
