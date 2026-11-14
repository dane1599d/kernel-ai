//! The restart (backlog item 13): the person's own `restart` asks, an Allow
//! whose ONCE checks restarts after the log is saved, and nothing else restarts.

use super::*;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

const REBOOT: &str = r#"{"name":"power_reboot","arguments":{}}"#;

#[test]
fn a_typed_restart_asks_and_a_no_does_not_restart() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let asked = shell.eval("restart");
    assert!(asked.confirm.is_some(), "{asked:?}");
    assert!(asked.text.contains("power_reboot()"), "{asked:?}");
    assert!(asked.text.contains("Restart the computer"), "{asked:?}");
    shell.eval("n");
    assert!(host.reboots.borrow().is_empty());
    // A bare y is no Allow: the restart's question needs the trusted path.
    let asked = shell.eval("please reboot the computer");
    let id = asked.confirm.expect("asks");
    shell.eval(&format!("answer {id} y"));
    assert!(host.reboots.borrow().is_empty());
}

#[test]
fn an_allow_saves_the_log_then_restarts() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let id = shell.eval("reboot").confirm.expect("asks");
    let once = host.mint(id, "power_reboot()");
    let answered = shell.eval(&format!("answer {id} allow once={once}"));
    assert_eq!(answered, ok("Restarting the computer"));
    let reboots = host.reboots.borrow();
    assert_eq!(reboots.len(), 1);
    let log = reboots[0].as_deref().unwrap_or_default();
    assert!(log.contains("power_reboot"), "{log}");
}

#[test]
fn a_question_about_restarting_never_restarts() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    // The model reads a question and calls the row anyway: it is refused, and
    // nothing asks.
    for line in ["how do I restart?", "tell me how to restart the computer"] {
        host.calls.borrow_mut().push_back(call(REBOOT));
        let reply = shell.eval(line);
        assert!(reply.confirm.is_none(), "{line}: {reply:?}");
        assert!(!reply.ok, "{line}: {reply:?}");
    }
    assert!(host.reboots.borrow().is_empty());
}

#[test]
fn a_model_call_to_restart_is_refused_even_for_a_plain_line() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    host.calls.borrow_mut().push_back(call(REBOOT));
    let reply = shell.eval("the computer feels slow, start it fresh");
    assert!(reply.confirm.is_none(), "{reply:?}");
    assert!(reply.text.contains("typed by the person"), "{reply:?}");
    assert!(host.reboots.borrow().is_empty());
}

#[test]
fn a_rhai_variable_named_restart_is_read_not_restarted() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(shell.eval("let restart = 3"), ok(""));
    assert_eq!(shell.eval("restart"), ok("3"));
    assert!(host.reboots.borrow().is_empty());
}

/// The Terminal app's `shell_line` effect for a person's `text`.
fn typed_in_terminal_app(shell: &mut Shell, text: &str) -> Result<String, String> {
    let args = format!("{{line: \"{text}\"}}");
    let effect = format!(
        "effect 1 call({{name: \"shell_line\", args: \"{}\"}})",
        args.replace('"', "\\\"")
    );
    let done = shell.run_effects_from(crate::appline::TERMINAL_APP, &effect, true);
    assert_eq!(done.len(), 1);
    done[0].outcome.clone()
}

/// A restart typed at the desk panel or in the Terminal window asks on the
/// trusted card (#864): a typed yes answers nothing, Don't restarts nothing,
/// and only the card's Allow, its ONCE checked, restarts.
#[test]
fn a_restart_from_the_panel_or_the_terminal_window_asks_on_the_card() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let shown = shell.eval_panel("restart");
    assert!(
        matches!(&shown, crate::PanelResult::Done(line) if line.contains("on the card")),
        "{shown:?}"
    );
    shell.eval_panel("yes");
    shell.eval("yes");
    assert!(host.reboots.borrow().is_empty());
    // Don't, on the card.
    let card = shell.eval_console(termcore::menu::TAKE_CARD);
    let menu = termcore::menu::Menu::decode(&card.to_wire()).unwrap();
    assert_eq!(menu.call(), "power_reboot()");
    let once = host.mint(menu.id, menu.call());
    shell.eval_console(&format!("answer {} deny once={once}", menu.id));
    assert!(host.reboots.borrow().is_empty());
    // From the Terminal window: Allow, on the card, restarts.
    let said = typed_in_terminal_app(&mut shell, "reboot").unwrap();
    assert!(said.contains("on the card"), "{said}");
    assert!(host.reboots.borrow().is_empty());
    let card = shell.eval_console(termcore::menu::TAKE_CARD);
    assert!(card.to_wire().contains(" added\n"), "{card:?}");
    let menu = termcore::menu::Menu::decode(&card.to_wire()).unwrap();
    let once = host.mint(menu.id, menu.call());
    let allowed = shell.eval_console(&format!("answer {} allow once={once}", menu.id));
    assert!(allowed.ok, "{allowed:?}");
    assert_eq!(host.reboots.borrow().len(), 1);
}
