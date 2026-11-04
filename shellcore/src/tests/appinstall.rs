//! The install card ("Kai builds apps", piece 4): `install it`, `install
//! <name>` and the act model's `app_install` ask on the trusted path's menu,
//! bound to the build and its source; only an Allow whose `ONCE` checks
//! installs, and `undo` switches back.

use super::*;
use crate::ccbuild::CcBuilt;
use alloc::vec;
use llmproto::script::{Answer, AnswerError};

const SOURCE: &str = "#include <kaiapp.h>\nstruct state { I64 left; };\n";
const TIMER: &str =
    r#"{"name":"app_build","arguments":{"name":"timer","request":"a countdown timer"}}"#;
const INSTALL: &str = r#"{"name":"app_install","arguments":{"name":"timer"}}"#;

fn app() -> Result<Answer, AnswerError> {
    Ok(Answer {
        summary: String::from("A countdown timer."),
        script: String::from(SOURCE),
    })
}

/// A shell that has built the timer, as entry 1.
fn built() -> (Rc<FakeHost>, Shell) {
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
        provenance: String::from(
            "built-by cc 631e318432aeaaf72d6d2dfc93085766934314a2 source 5e1f",
        ),
        path: String::from("/home/user/apps/timer.kai"),
    }));
    *host.cc_imports.borrow_mut() = vec![
        String::from("kai:app/log@0.1.0"),
        String::from("kai:ui/view@0.1.0"),
    ];
    let mut shell = shell(&host);
    let done = shell.eval("make me a countdown timer app");
    assert!(
        done.ok
            && done
                .text
                .ends_with("`show code` shows it, and `install timer` asks to install it."),
        "{done:?}"
    );
    (host, shell)
}

/// The card's call for the source the host holds now.
fn call_for(host: &FakeHost) -> String {
    let sha = crate::appbuild::sha256(&host.app_sources.borrow()[0].1);
    format!("app_install(\"timer\", \"0.0.1\", \"{}\")", &sha[..16])
}

fn locals(host: &FakeHost) -> usize {
    host.pkg_sent
        .borrow()
        .iter()
        .filter(|s| s.starts_with("local "))
        .count()
}

#[test]
fn install_timer_asks_on_the_card_and_an_allow_installs_it_with_its_undo() {
    let (host, mut shell) = built();
    let call = call_for(&host);
    let card = shell.eval("install timer");
    let menu = termcore::menu::Menu::decode(&card.to_wire()).expect("a menu");
    assert_eq!(menu.id, 2);
    assert_eq!(menu.call(), call);
    assert_eq!(
        menu.question(),
        format!("Install Kai's app, 9 KiB, using app/log, ui/view, with {call}?")
    );
    assert_eq!(locals(&host), 0, "nothing installs before the Allow");
    host.pkg_answers
        .borrow_mut()
        .push_back(Ok(String::from("timer 0.0.1 generation 5 from 4")));
    let once = host.mint(2, &call);
    let done = shell.eval(&format!("answer 2 allow once={once}"));
    assert_eq!(
        done,
        ok(
            "Installed Kai's app timer 0.0.1 as generation 5; it opens now. undo removes it, and its source stays in /home/user/apps/timer/."
        )
    );
    assert_eq!(
        host.pkg_sent.borrow().last().unwrap(),
        "local timer /home/user/apps/timer.kai"
    );
    let log = shell.eval("log actlog").text;
    assert!(log.contains("app_install owner=pkg"), "{log}");
    assert!(log.contains("inv=local:5:4"), "{log}");
    assert!(log.contains("consent=yes"), "{log}");
    // Installed once: another card is refused, with why.
    let again = shell.eval("install it");
    assert!(
        !again.ok && again.text.contains("is installed already"),
        "{again:?}"
    );
    answers_switch_back(&host);
    assert_eq!(
        shell.eval("undo"),
        ok("Undone: packages back to generation 4")
    );
    assert_eq!(host.pkg_sent.borrow().last().unwrap(), "switch-back 4 5");
    assert_eq!(host.app_sources.borrow().len(), 1, "the source stays");
    // Undone, the same build may be asked about again.
    let after = shell.eval("install it");
    assert!(after.to_wire().starts_with("menu "), "{after:?}");
}

fn answers_switch_back(host: &FakeHost) {
    host.pkg_answers
        .borrow_mut()
        .push_back(Ok(String::from("generation 6")));
}

#[test]
fn install_it_and_the_act_model_s_app_install_open_the_same_card() {
    let (host, mut shell) = built();
    let call = call_for(&host);
    let card = shell.eval("install it");
    assert_eq!(
        termcore::menu::Menu::decode(&card.to_wire())
            .unwrap()
            .call(),
        call
    );
    host.calls
        .borrow_mut()
        .push_back(Ok(llmproto::act::parse(INSTALL).unwrap()));
    // The model's call asks too, even for a line that names it plainly.
    let card = shell.eval("please install my timer app");
    let menu = termcore::menu::Menu::decode(&card.to_wire()).expect("a menu");
    assert_eq!(menu.call(), call);
    assert_eq!(locals(&host), 0);
}

#[test]
fn only_an_allow_whose_once_names_this_card_installs() {
    let (host, mut shell) = built();
    let call = call_for(&host);
    shell.eval("install timer");
    // A typed yes cannot answer a menu.
    let typed = shell.eval("y");
    assert!(typed.text.contains("not run"), "{typed:?}");
    shell.eval("install timer");
    // A ONCE minted for another card's id does not check.
    let wrong = host.mint(2, &call);
    let refused = shell.eval(&format!("answer 3 allow once={wrong}"));
    assert!(refused.text.contains("did not check"), "{refused:?}");
    shell.eval("install timer");
    let denied = shell.eval("answer 4 deny");
    assert!(denied.text.contains("not run"), "{denied:?}");
    assert_eq!(locals(&host), 0);
}

#[test]
fn an_edited_or_replaced_build_is_refused_with_why() {
    let (host, mut shell) = built();
    let call = call_for(&host);
    // Edited before the card: refused at once.
    host.app_sources.borrow_mut()[0].1 = String::from("/* mine */\n");
    let edited = shell.eval("install timer");
    assert!(
        !edited.ok
            && edited
                .text
                .contains("was changed since Kai built timer 0.0.1"),
        "{edited:?}"
    );
    // Edited while the card was up: the Allow installs nothing.
    host.app_sources.borrow_mut()[0].1 = kai_source();
    shell.eval("install timer");
    host.app_sources.borrow_mut()[0].1 = String::from("/* mine */\n");
    let once = host.mint(3, &call);
    let late = shell.eval(&format!("answer 3 allow once={once}"));
    assert!(
        !late.ok && late.text.contains("was changed since Kai built"),
        "{late:?}"
    );
    assert_eq!(locals(&host), 0);
    assert!(shell.eval("log actlog").text.contains("refused:"));
}

/// The `main.c` Kai wrote, as the build recorded it.
fn kai_source() -> String {
    llmproto::precheck::prepare(SOURCE, &[]).source
}

#[test]
fn install_of_a_name_kai_did_not_build_is_a_package_line() {
    let (host, mut shell) = built();
    let reply = shell.eval("install notes");
    assert!(!reply.text.starts_with("menu "), "{reply:?}");
    assert!(!reply.text.contains("Kai's app"), "{reply:?}");
    assert_eq!(locals(&host), 0);
    let fresh = FakeHost::new();
    let mut none = super::shell(&fresh);
    assert_eq!(
        none.eval("install it"),
        Reply::err("Kai has built no app to install")
    );
}

#[test]
fn show_code_shows_main_c_as_it_is_now() {
    let (host, mut shell) = built();
    let shown = shell.eval("show code");
    assert!(
        shown.ok
            && shown
                .text
                .starts_with("/home/user/apps/timer/main.c:\n#include"),
        "{shown:?}"
    );
    assert_eq!(shell.eval("show code timer"), shown);
    assert!(!shell.eval("show code clock").ok);
    assert_eq!(host.app_sources.borrow().len(), 1);
}
