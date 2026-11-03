//! The editor's build door in the shell (P4): Ctrl-B on a C component runs
//! `cc build` as a typed line does, logged as the editor's and undone the same
//! way; only the editor may, only for a person's key, and no script can reach it.

use super::*;
use crate::ccbuild::CcBuilt;
use crate::door::DOOR_APP;
use llmproto::script::Answer;

const PROVENANCE: &str = "built-by cc 631e318432aeaaf72d6d2dfc93085766934314a2 source 5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f5e1f";

fn built() -> CcBuilt {
    CcBuilt {
        app: true,
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
fn a_build_is_the_typed_lines_install_logged_as_the_editor_s_and_undone_the_same_way() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    answers(&host, &[Ok("0.0.3"), Ok("hello 0.0.3 generation 5 from 4")]);
    *host.cc_built.borrow_mut() = Some(Ok(built()));
    let report = shell.door_build(DOOR_APP, "hello", true).unwrap();
    assert_eq!(
        report,
        "ok built hello 0.0.3 (5120 bytes) and installed it as generation 5. undo"
    );
    assert_eq!(
        *host.pkg_sent.borrow(),
        [
            "local-next hello",
            "cc_build hello 0.0.3",
            "local hello /home/user/c/hello.kai"
        ]
    );
    // One entry: the editor's, with D7's provenance, and the typed line's undo.
    let log = shell.eval("log actlog").text;
    assert_eq!(log.matches("editor: build hello").count(), 1, "{log}");
    assert!(log.contains("app:editor"), "{log}");
    assert!(log.contains("inv=local:5:4"), "{log}");
    assert!(host.audit.borrow().iter().any(|a| a.ends_with(PROVENANCE)));
    answers(&host, &[Ok("generation 6")]);
    assert_eq!(
        shell.eval("undo"),
        ok("Undone: packages back to generation 4")
    );
}

#[test]
fn a_build_without_a_person_s_key_or_click_builds_nothing() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    let writes = host.log_writes.get();
    let refused = shell.door_build(DOOR_APP, "hello", false);
    assert!(refused.unwrap_err().contains("key or click"));
    assert!(
        host.pkg_sent.borrow().is_empty(),
        "pkg and cc heard nothing"
    );
    assert_eq!(host.log_writes.get(), writes, "no log entry");
    assert_eq!(shell.eval("undo"), Reply::err("nothing to undo"));
}

#[test]
fn only_the_editor_uses_the_build_door() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    for app in [
        "notes",
        "files",
        "terminal-window",
        "shell",
        "editor-window",
        "",
    ] {
        let refused = shell.door_build(app, "hello", true);
        assert!(
            refused.unwrap_err().contains("does not use the file door"),
            "{app}"
        );
    }
    assert!(host.pkg_sent.borrow().is_empty());
}

#[test]
fn a_name_that_is_not_a_components_is_an_error_report_that_reaches_neither_pkg_nor_cc() {
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
        "",
    ] {
        let report = shell.door_build(DOOR_APP, name, true).unwrap();
        assert!(
            report.starts_with("err cc build takes a component's name"),
            "{name}: {report}"
        );
    }
    assert!(host.pkg_sent.borrow().is_empty());
}

/// The editor's pager shows `cc`'s lines one to a line; anything but printable
/// ASCII and the line breaks is still cut to `?`, and the text is cut.
#[test]
fn the_compilers_diagnostics_keep_their_lines_for_the_pager_but_not_control_characters() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    answers(&host, &[Ok("0.0.1")]);
    *host.cc_built.borrow_mut() = Some(Err(String::from(
        "hello.c:3:5: error: x\n  3 | int y\u{1b}[2J\nhello.c:7:1: error: z",
    )));
    let report = shell.door_build(DOOR_APP, "hello", true).unwrap();
    assert_eq!(
        report,
        "err cc build hello: hello.c:3:5: error: x\n  3 | int y?[2J\nhello.c:7:1: error: z"
    );
    // Nothing was installed, so nothing is logged or undone.
    assert_eq!(shell.eval("undo"), Reply::err("nothing to undo"));

    answers(&host, &[Ok("0.0.1")]);
    *host.cc_built.borrow_mut() = Some(Err("e\n".repeat(5_000)));
    let long = shell.door_build(DOOR_APP, "hello", true).unwrap();
    assert!(long.len() < 4_100, "{}", long.len());
}

/// A typed line still sees `cc`'s text as one printable line, as before.
#[test]
fn a_typed_line_still_gets_the_compilers_text_on_one_line() {
    let host = FakeHost::new();
    let mut shell = shell(&host);
    answers(&host, &[Ok("0.0.1")]);
    *host.cc_built.borrow_mut() = Some(Err(String::from("a.c: 1\nb.c: 2")));
    assert_eq!(
        shell.eval("cc build hello"),
        Reply::err("cc build hello: a.c: 1?b.c: 2")
    );
}

#[test]
fn no_script_can_reach_the_build_door() {
    let host = FakeHost::new();
    let dyn_host: Rc<dyn Host> = host.clone();
    for source in [
        "door_build(\"hello\");",
        "editor_build(\"hello\");",
        "cc_build(\"hello\", \"0.0.1\");",
    ] {
        let answer = Answer {
            summary: String::new(),
            script: String::from(source),
        };
        let checked = script::check(&answer, &dyn_host, &DEFAULT_CONFIG);
        assert!(checked.is_err(), "{source} was accepted");
    }
    assert!(host.pkg_sent.borrow().is_empty());
}
