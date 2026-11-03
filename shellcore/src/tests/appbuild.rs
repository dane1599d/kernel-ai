//! `app_build` ("Kai builds apps", piece 3) against a fake model, `pkg` and
//! `cc`: the source kept in `/home/user/apps/<name>/`, built at `local-next`'s
//! version, repaired with `cc`'s exact errors, logged with its hash and
//! provenance, and never installed.

use super::*;
use crate::ccbuild::CcBuilt;
use alloc::vec;
use llmproto::act::{ActError, Call};
use llmproto::script::{Answer, AnswerError, Lang};

const PROVENANCE: &str = "built-by cc 631e318432aeaaf72d6d2dfc93085766934314a2 source 5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f";
const SOURCE: &str = "#include <kaiapp.h>\n#include <string.h>\nstruct state { I64 left; };\nvoid init(struct state *s) { s->left = 60; }\n";
const TIMER: &str =
    r#"{"name":"app_build","arguments":{"name":"timer","request":"a countdown timer"}}"#;

fn call(json: &str) -> Result<Call, ActError> {
    Ok(llmproto::act::parse(json).unwrap())
}

fn app(script: &str) -> Result<Answer, AnswerError> {
    Ok(Answer {
        summary: String::from("A countdown timer."),
        script: String::from(script),
    })
}

fn built() -> CcBuilt {
    CcBuilt {
        app: true,
        bytes: 9_216,
        provenance: String::from(PROVENANCE),
        path: String::from("/home/user/apps/timer.kai"),
    }
}

/// A shell whose model calls `app_build` for the timer, then writes `cards`.
fn asked(cards: Vec<Result<Answer, AnswerError>>) -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    host.calls.borrow_mut().push_back(call(TIMER));
    host.answers.borrow_mut().extend(cards);
    let shell = shell(&host);
    (host, shell)
}

#[test]
fn an_app_is_written_built_and_logged_with_its_source_hash_but_not_installed() {
    let (host, mut shell) = asked(vec![app(SOURCE)]);
    host.pkg_answers
        .borrow_mut()
        .push_back(Ok(String::from("0.0.1")));
    host.cc_builds.borrow_mut().push_back(Ok(built()));
    let reply = shell.eval("make me a countdown timer app");
    assert!(
        reply.ok
            && reply
                .text
                .contains("Built timer 0.0.1 (9216 bytes): A countdown timer."),
        "{reply:?}"
    );
    // The card got the name and the request, as an app request.
    let asked = host.script_requests.borrow().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].lang, Lang::App);
    assert_eq!(asked[0].request, "timer\na countdown timer");
    // The source is kept where "Show code" finds it, then built from there.
    assert_eq!(
        *host.app_sources.borrow(),
        [(String::from("timer"), String::from(SOURCE))]
    );
    assert_eq!(
        *host.pkg_sent.borrow(),
        ["local-next timer", "cc_build /home/user/apps/timer 0.0.1"]
    );
    // Logged with the source's hash and cc's provenance; nothing went to local.
    let actlog = shell.eval("log actlog").text;
    let hash = crate::appbuild::sha256(SOURCE);
    assert!(actlog.contains(&hash), "{actlog}");
    assert!(actlog.contains(PROVENANCE), "{actlog}");
    assert!(actlog.contains("not_installed"), "{actlog}");
    assert!(
        !host
            .pkg_sent
            .borrow()
            .iter()
            .any(|s| s.starts_with("local "))
    );
}

#[test]
fn a_build_cc_refuses_is_repaired_with_its_errors_then_given_up_after_three() {
    let cards = (0..4).map(|_| app(SOURCE)).collect();
    let (host, mut shell) = asked(cards);
    for _ in 0..4 {
        host.pkg_answers
            .borrow_mut()
            .push_back(Ok(String::from("0.0.1")));
        host.cc_builds
            .borrow_mut()
            .push_back(Err(String::from("main.c:4: error: 'tick' undeclared")));
    }
    let reply = shell.eval("make me a countdown timer app");
    assert!(
        !reply.ok && reply.text.contains("did not build after 3 repairs"),
        "{reply:?}"
    );
    let asked = host.script_requests.borrow().clone();
    assert_eq!(asked.len(), 4);
    assert_eq!(asked[0].repair, None);
    // Each repair has the source as written and cc's exact error.
    assert_eq!(
        asked[1].repair,
        Some((
            String::from(SOURCE),
            String::from("cc build refused it:\nmain.c:4: error: 'tick' undeclared")
        ))
    );
    // The failure is logged, and nothing was built to install.
    assert!(shell.eval("log actlog").text.contains("failed:"));
    assert!(shell.app_built.is_none());
}

#[test]
fn a_repaired_build_is_kept_once_it_builds() {
    let (host, mut shell) = asked(vec![app(SOURCE), app(SOURCE)]);
    let mut pkg = host.pkg_answers.borrow_mut();
    pkg.push_back(Ok(String::from("0.0.1")));
    pkg.push_back(Ok(String::from("0.0.1")));
    drop(pkg);
    let mut builds = host.cc_builds.borrow_mut();
    builds.push_back(Err(String::from("main.c:2: error: expected ';'")));
    builds.push_back(Ok(built()));
    drop(builds);
    let reply = shell.eval("make me a countdown timer app");
    assert!(reply.ok, "{reply:?}");
    assert_eq!(host.script_requests.borrow().len(), 2);
    let kept = shell.app_built.clone().expect("a build waits for its card");
    assert_eq!(
        (kept.name.as_str(), kept.version.as_str()),
        ("timer", "0.0.1")
    );
    assert_eq!(kept.source, crate::appbuild::sha256(SOURCE));
}

#[test]
fn a_bad_name_or_no_card_builds_nothing() {
    let bad = r#"{"name":"app_build","arguments":{"name":"Timer!","request":"a timer"}}"#;
    let host = FakeHost::new();
    host.calls.borrow_mut().push_back(call(bad));
    let mut shell = shell(&host);
    let reply = shell.eval("make me a timer app");
    assert!(
        !reply.ok && reply.text.contains("an app's name"),
        "{reply:?}"
    );
    assert!(host.script_requests.borrow().is_empty());
    // Until the c-app-1 card is there, the llm service says so, and nothing is kept.
    let no_card = AnswerError::NoScript(String::from(llmproto::script::NO_APP_CARD));
    let (host, mut shell) = asked(vec![Err(no_card)]);
    let reply = shell.eval("make me a countdown timer app");
    assert!(
        !reply.ok && reply.text.contains("the c-app-1 card is not here yet"),
        "{reply:?}"
    );
    assert!(host.app_sources.borrow().is_empty());
}
