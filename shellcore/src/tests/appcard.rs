//! The install card that opens by itself ("Kai builds apps", follow-up 2): a
//! build in the background that ends built leaves its card waiting, the host
//! says so (CARD_READY), and the kernel's terminal takes it with `TAKE_CARD`:
//! the same card, checks and `ONCE` as `install <name>`.

use super::*;
use crate::ccbuild::CcBuilt;
use alloc::vec;
use llmproto::script::{Answer, AnswerError};
use termcore::menu::{Menu, TAKE_CARD};

const SOURCE: &str = "#include <kaiapp.h>\nstruct state { I64 left; };\n";
const TIMER: &str =
    r#"{"name":"app_build","arguments":{"name":"timer","request":"a countdown timer"}}"#;

fn app() -> Result<Answer, AnswerError> {
    Ok(Answer {
        summary: String::from("A countdown timer."),
        script: String::from(SOURCE),
    })
}

/// A shell on a host that builds off the line, which has built the timer in
/// the background: its build ended, and nothing was typed since.
fn built_in_background() -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    host.background.set(true);
    host.calls
        .borrow_mut()
        .push_back(Ok(llmproto::act::parse(TIMER).unwrap()));
    host.answers.borrow_mut().extend(vec![app()]);
    host.pkg_answers
        .borrow_mut()
        .push_back(Ok(String::from("0.0.1")));
    host.cc_builds.borrow_mut().push_back(Ok(CcBuilt {
        app: true,
        bytes: 9_216,
        provenance: String::from("built-by cc 631e3184 source 5e1f"),
        path: String::from("/home/user/apps/timer.kai"),
    }));
    let mut shell = shell(&host);
    let started = shell.eval_console("make me a countdown timer app");
    assert!(started.text.starts_with("Building timer:"), "{started:?}");
    assert!(!shell.card_waiting(), "no card before the build ends");
    host.answered.set(true);
    assert!(matches!(
        shell.app_poll(),
        Some(crate::PanelResult::Done(_))
    ));
    (host, shell)
}

fn call_for(host: &FakeHost) -> String {
    let sha = crate::appbuild::sha256(&host.app_sources.borrow()[0].1);
    format!("app_install(\"timer\", \"0.0.1\", \"{}\")", &sha[..16])
}

#[test]
fn a_build_that_ends_built_leaves_its_card_for_the_terminal_to_take() {
    let (host, mut shell) = built_in_background();
    assert!(shell.card_waiting());
    let card = shell.eval_console(TAKE_CARD);
    let menu = Menu::decode(&card.to_wire()).expect("the install card");
    assert_eq!(menu.call(), call_for(&host));
    assert!(
        menu.question().contains("using app/log, ui/view"),
        "{menu:?}"
    );
    // Taken once: the next asks for none.
    assert!(!shell.card_waiting());
    assert_eq!(shell.eval_console(TAKE_CARD).to_wire(), "ok ");
    // Its Allow installs, as the typed card's does.
    host.pkg_answers
        .borrow_mut()
        .push_back(Ok(String::from("timer 0.0.1 generation 5 from 4")));
    let once = host.mint(menu.id, menu.call());
    let done = shell.eval_console(&format!("answer {} allow once={once}", menu.id));
    assert!(done.ok && done.text.contains("as generation 5"), "{done:?}");
    assert!(
        shell
            .eval("log actlog")
            .text
            .contains("the card opened when the build ended"),
        "the log says how the card came"
    );
}

#[test]
fn a_typed_allow_or_a_deny_of_the_card_installs_nothing() {
    let (host, mut shell) = built_in_background();
    let menu = Menu::decode(&shell.eval_console(TAKE_CARD).to_wire()).unwrap();
    let typed = shell.eval_console(&format!("answer {} allow", menu.id));
    assert!(typed.text.contains("not run"), "{typed:?}");
    assert!(
        !host
            .pkg_sent
            .borrow()
            .iter()
            .any(|s| s.starts_with("local ")),
        "no install without the card's ONCE"
    );
}

#[test]
fn the_card_waits_while_another_ask_is_up_and_a_typed_install_takes_its_place() {
    let (_host, mut shell) = built_in_background();
    // Another ask is up: the card waits on.
    let asked = shell.eval_console("install timer");
    assert!(asked.to_wire().starts_with("menu "), "{asked:?}");
    assert!(!shell.card_waiting(), "the typed install is the card");
    assert_eq!(shell.eval_console(TAKE_CARD).to_wire(), "ok ");
}

#[test]
fn a_card_whose_build_was_edited_since_is_refused_with_why() {
    let (host, mut shell) = built_in_background();
    host.app_sources.borrow_mut().push((
        String::from("timer"),
        String::from("int main(void) { return 1; }"),
    ));
    let card = shell.eval_console(TAKE_CARD);
    assert!(!card.ok && card.text.contains("was changed"), "{card:?}");
    assert!(!shell.card_waiting());
}

#[test]
fn take_card_is_the_terminal_s_alone_and_a_build_within_a_line_leaves_none() {
    let (_host, mut waiting) = built_in_background();
    // The panel's and an app's lines go through `eval`: no card from there.
    let panel = waiting.eval_panel(TAKE_CARD);
    assert!(!matches!(&panel, crate::PanelResult::Done(line) if line.starts_with("menu")));
    assert!(waiting.card_waiting(), "still waiting for the terminal");
    // A build within the line asks with its own `install timer` hint.
    let host = FakeHost::new();
    host.calls
        .borrow_mut()
        .push_back(Ok(llmproto::act::parse(TIMER).unwrap()));
    host.answers.borrow_mut().extend(vec![app()]);
    host.pkg_answers
        .borrow_mut()
        .push_back(Ok(String::from("0.0.1")));
    host.cc_builds.borrow_mut().push_back(Ok(CcBuilt {
        app: true,
        bytes: 9_216,
        provenance: String::from("built-by cc 631e3184 source 5e1f"),
        path: String::from("/home/user/apps/timer.kai"),
    }));
    let mut inline = shell(&host);
    assert!(inline.eval_console("make me a countdown timer app").ok);
    assert!(!inline.card_waiting());
}
