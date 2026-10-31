//! `cc build <component>` against a fake `pkg` and `cc`: the version from
//! `local-next`, an app installed from `local` and its undo through
//! `switch-back`, a component built only, and hostile names and answers.

use super::*;
use crate::ccbuild::CcBuilt;

const PROVENANCE: &str = "built-by cc 631e318432aeaaf72d6d2dfc93085766934314a2 source 5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f";

fn built(app: bool) -> CcBuilt {
    CcBuilt {
        app,
        bytes: 5_120,
        provenance: String::from(PROVENANCE),
        path: String::from("/home/user/c/hello.kai"),
    }
}

fn answers(host: &FakeHost, answers: &[Result<&str, &str>]) {
    let mut queue = host.pkg_answers.borrow_mut();
    for answer in answers {
        queue.push_back(answer.map(String::from).map_err(String::from));
    }
}

#[test]
fn an_app_is_built_installed_from_local_and_undone_by_switching_back() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    answers(&host, &[Ok("0.0.3"), Ok("hello 0.0.3 generation 5 from 4")]);
    *host.cc_built.borrow_mut() = Some(Ok(built(true)));
    assert_eq!(
        shell.eval("cc build hello"),
        ok("built hello 0.0.3 (5120 bytes) and installed it as generation 5. undo")
    );
    assert_eq!(
        *host.pkg_sent.borrow(),
        [
            "local-next hello",
            "cc_build hello 0.0.3",
            "local hello /home/user/c/hello.kai"
        ]
    );
    // D7's provenance: in the audit and in the entry's script.
    assert!(host.audit.borrow().iter().any(|a| a.ends_with(PROVENANCE)));
    let log = shell.eval("log actlog").text;
    assert!(log.contains("res=pkg-generation before=4 after=5"), "{log}");
    assert!(log.contains("cc_build owner=pkg"), "{log}");
    assert!(log.contains("inv=local:5:4"), "{log}");
    answers(&host, &[Ok("generation 6")]);
    assert_eq!(
        shell.eval("undo"),
        ok("Undone: packages back to generation 4")
    );
    assert_eq!(host.pkg_sent.borrow().last().unwrap(), "switch-back 4 5");
    // pkg goes back to the parent only: the undo is not redone.
    let again = shell.eval("undo");
    assert!(
        again.text.contains("installed again by cc build"),
        "{again:?}"
    );
}

#[test]
fn it_works_at_the_c_prompt_too() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert!(shell.eval(":c").text.starts_with("C mode"));
    answers(&host, &[Ok("0.0.1"), Ok("hello 0.0.1 generation 2 from 1")]);
    *host.cc_built.borrow_mut() = Some(Ok(built(true)));
    let reply = shell.eval("cc build hello");
    assert!(reply.text.starts_with("built hello 0.0.1"), "{reply:?}");
}

#[test]
fn a_component_is_built_but_not_installed() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    answers(&host, &[Ok("0.0.1")]);
    *host.cc_built.borrow_mut() = Some(Ok(built(false)));
    assert_eq!(
        shell.eval("cc build hello"),
        ok(
            "built hello 0.0.1, a component of 5120 bytes: /home/user/c/hello.kai. Only an app installs from local."
        )
    );
    assert_eq!(host.pkg_sent.borrow().len(), 2);
    assert_eq!(shell.eval("undo"), Reply::err("nothing to undo"));
}

#[test]
fn a_name_that_is_not_a_components_reaches_neither_pkg_nor_cc() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    for name in [
        "../x",
        "a/b",
        "Hello",
        "h",
        "hello-world-12",
        "x y",
        "1abc",
        "a\\b",
        ".hidden",
        "",
    ] {
        let reply = shell.eval(&format!("cc build {name}"));
        assert!(
            reply.text.contains("takes a component's name"),
            "{name}: {reply:?}"
        );
    }
    assert!(host.pkg_sent.borrow().is_empty());
}

#[test]
fn failures_say_where_the_build_stopped() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    assert_eq!(
        shell.eval("cc build hello"),
        Reply::err("cc build hello: pkg: no pkg here")
    );
    answers(&host, &[Ok("0.0.1")]);
    *host.cc_built.borrow_mut() = Some(Err(String::from("main.c: 2: syntax error")));
    assert_eq!(
        shell.eval("cc build hello"),
        Reply::err("cc build hello: main.c: 2: syntax error")
    );
    answers(
        &host,
        &[Ok("0.0.1"), Err("hello is a package the image holds")],
    );
    *host.cc_built.borrow_mut() = Some(Ok(built(true)));
    assert_eq!(
        shell.eval("cc build hello"),
        Reply::err(
            "built hello 0.0.1 at /home/user/c/hello.kai, but local did not install it: hello is a package the image holds"
        )
    );
    assert_eq!(shell.eval("undo"), Reply::err("nothing to undo"));
}

/// An answer from `pkg` out of shape installs nothing the shell can undo, and
/// says so rather than guess a generation.
#[test]
fn a_local_answer_out_of_shape_gets_no_undo() {
    for answer in [
        "hello 0.0.1 generation 5",
        "other 0.0.1 generation 5 from 4",
        "hello 0.0.1 generation five from 4",
        "hello 0.0.1 generation 5 from 4 extra",
        "hello 0.0.9 generation 5 from 4",
    ] {
        let host = FakeHost::new();
        let mut shell = shell(&host);
        answers(&host, &[Ok("0.0.1"), Ok(answer)]);
        *host.cc_built.borrow_mut() = Some(Ok(built(true)));
        let reply = shell.eval("cc build hello");
        assert!(reply.text.contains("gives no undo"), "{answer}: {reply:?}");
        let undo = shell.eval("undo");
        assert!(!undo.text.starts_with("Undone"), "{answer}: {undo:?}");
    }
}

/// Security review: `local` that gives no answer may still have installed,
/// so the attempt is logged, with nothing `undo` would switch back.
#[test]
fn a_local_that_never_answers_is_logged_without_an_undo() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    answers(&host, &[Ok("0.0.1"), Err("pkg did not answer (TimedOut)")]);
    *host.cc_built.borrow_mut() = Some(Ok(built(true)));
    let reply = shell.eval("cc build hello");
    assert!(reply.text.contains("it may have installed it"), "{reply:?}");
    let log = shell.eval("log actlog").text;
    assert!(log.contains("state=nothing"), "{log}");
    assert!(
        log.contains("local: pkg did not answer (TimedOut)"),
        "{log}"
    );
    assert_eq!(shell.eval("undo"), Reply::err("nothing to undo"));
}

#[test]
fn ccs_reply_is_read_strictly() {
    let path = || String::from("/home/user/c/hello.kai");
    let good = format!("app 5120 {PROVENANCE}");
    assert_eq!(CcBuilt::from_reply(&good, path()), Ok(built(true)));
    for bad in [
        "",
        "app",
        "app 5120",
        "app x built-by cc a source b",
        "library 5120 built-by cc a source b",
        "app 5120 made-by cc",
        "app -1 built-by cc a source b",
        "app 5120 built-by cc 631e3184 source 5e1f",
        "app 5120 built-by cc 631e318432aeaaf72d6d2dfc93085766934314a2 source 5E1F",
        "app 5120 built-by cc 631e318432aeaaf72d6d2dfc93085766934314a2 source x\nforged",
    ] {
        assert!(CcBuilt::from_reply(bad, path()).is_err(), "{bad:?}");
    }
    let long = format!("app 5120 {PROVENANCE}0");
    assert!(CcBuilt::from_reply(&long, path()).is_err());
    assert_eq!(
        built(true).source(),
        "5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f"
    );
}
