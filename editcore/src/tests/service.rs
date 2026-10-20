use super::fake::{COMMIT, Fake, Host, TOKEN, kernel, opened};
use super::*;
use crate::service::Service;
use kabi::Error;

#[test]
fn a_good_build_uploads_exactly_what_came_back() {
    let (mut service, mut env) = opened("let a = 1;\nlet b = 2;\n");
    assert_eq!(
        kernel(&mut service, &mut env, "cmd s |b = 2|b = 3|"),
        format!("ok {LIB}:2: let b = 3;")
    );
    let reply = kernel(&mut service, &mut env, "cmd build");
    assert_eq!(
        reply,
        "ok built e1 from 2 (commit 1a2b3c4d5e6f); :apply runs it"
    );
    let (component, bytes, base, commit) = &env.puts[0];
    assert_eq!(
        (component.as_str(), base.as_str(), commit.as_str()),
        ("notes", "2", COMMIT)
    );
    let parsed = livecore::artifact::parse(bytes).unwrap();
    assert_eq!(parsed.manifest.version, "e1");
    assert_eq!(parsed.source, small("let a = 1;\nlet b = 3;\n"));
    let request = String::from_utf8_lossy(&env.requests[0]).into_owned();
    assert!(request.starts_with("POST /build HTTP/1.1\r\nHost: 10.0.2.2:5000\r\n"));
    // The same text is not built twice.
    assert!(kernel(&mut service, &mut env, "cmd build").contains("built from this text already"));
    assert_eq!(env.requests.len(), 1);
}

#[test]
fn a_failed_build_stores_nothing_and_moves_to_the_first_error() {
    let (mut service, mut env) = opened("let a = 1;\nlet b = 2;\nlet c = 3;\n");
    kernel(&mut service, &mut env, "cmd s |c = 3|c = 4|");
    env.host = Host::Fails;
    let reply = kernel(&mut service, &mut env, "cmd build");
    assert!(
        reply.starts_with("err build failed; the cursor is on the first error"),
        "{reply}"
    );
    assert!(reply.contains("components/notes/src/lib.rs:2:5: error[E0277]"));
    assert!(env.puts.is_empty());
    assert_eq!(service.session.as_ref().unwrap().line(), 2);
    assert!(reply.len() <= kabi::MAX_MESSAGE);
}

#[test]
fn a_wrong_artifact_or_a_bad_reply_is_refused() {
    for (host, wanted) in [
        (
            Host::WrongLabel,
            "the wrong artifact: it is notes e9, not notes e1",
        ),
        (
            Host::WrongSource,
            "the wrong artifact: its source is not what was sent",
        ),
        (Host::Forbidden, "err build service: HTTP 403 bad token"),
        (Host::Garbage, "err build service: a malformed reply"),
    ] {
        let (mut service, mut env) = opened("x\n");
        kernel(&mut service, &mut env, "cmd c 1 y");
        env.host = host;
        let reply = kernel(&mut service, &mut env, "cmd build");
        assert!(reply.contains(wanted), "{reply}");
        assert!(env.puts.is_empty());
    }
}

#[test]
fn a_store_that_moved_on_says_to_build_again() {
    let (mut service, mut env) = opened("x\n");
    kernel(&mut service, &mut env, "cmd c 1 y");
    env.put_result = Err(Error::Invalid);
    let reply = kernel(&mut service, &mut env, "cmd build");
    assert!(reply.contains("build again"), "{reply}");
    assert_eq!(service.session.as_ref().unwrap().built, None);
}

#[test]
fn nothing_changed_and_rule_breaks_are_not_built() {
    let (mut service, mut env) = opened("x\n");
    assert_eq!(
        kernel(&mut service, &mut env, "cmd build"),
        "err nothing changed since notes 2"
    );
    kernel(
        &mut service,
        &mut env,
        r#"cmd c 1 const X: &str = include_str!("/etc/passwd");"#,
    );
    let reply = kernel(&mut service, &mut env, "cmd build");
    assert!(
        reply.starts_with("err not built:") && reply.contains("include_str"),
        "{reply}"
    );
    assert!(env.requests.is_empty());
}

#[test]
fn versions_whose_source_is_not_the_component_cannot_be_opened() {
    let mut service = Service::new("h", TOKEN);
    let mut env = Fake::new("x\n");
    let fixture = bundle(&[
        ("components/fixtures/notes-v1/Cargo.toml", b""),
        ("components/fixtures/notes-v1/src/lib.rs", b""),
    ]);
    env.artifacts.push(("1", artifact("notes", "1", &fixture)));
    let reply = kernel(&mut service, &mut env, "open notes 1");
    assert_eq!(
        reply,
        "err notes 1 is not editable: its source is not components/notes"
    );
    assert!(kernel(&mut service, &mut env, "open notes 7").contains("no version notes 7"));
    assert!(kernel(&mut service, &mut env, "open shell").contains("cannot be edited"));
    assert!(kernel(&mut service, &mut env, "cmd p").contains("no file is open"));
}

#[test]
fn only_the_kernel_is_answered() {
    let (mut service, mut env) = opened("x\n");
    assert_eq!(
        service.handle(&mut env, 4242, b"cmd build"),
        "err the edit service answers only the kernel"
    );
    assert!(env.requests.is_empty());
}

#[test]
fn reopening_the_same_version_resumes_the_session() {
    let (mut service, mut env) = opened("x\ny\n");
    kernel(&mut service, &mut env, "cmd c 2 z");
    assert_eq!(
        kernel(&mut service, &mut env, "open notes"),
        "ok editing notes 2 again, with your edits"
    );
    assert!(kernel(&mut service, &mut env, "cmd diff").contains("+ | z"));
}

#[test]
fn the_editor_commands_reply_with_what_they_did() {
    let (mut service, mut env) = opened(&numbered(30, "\n"));
    let files = kernel(&mut service, &mut env, "cmd files");
    assert!(files.contains(&format!(" > {LIB} (30 lines)")), "{files}");
    let page = kernel(&mut service, &mut env, "cmd p 5,7");
    assert_eq!(
        page,
        format!("ok {LIB} lines 5 to 7 of 30\n   5 | line 5\n   6 | line 6\n   7 | line 7")
    );
    let found = kernel(&mut service, &mut env, "cmd find line 2");
    assert!(found.starts_with("ok 11 lines hold \"line 2\""), "{found}");
    assert_eq!(service.session.as_ref().unwrap().line(), 2);
    assert_eq!(
        kernel(&mut service, &mut env, "cmd d 1,3"),
        format!("ok deleted 3 lines; {LIB} has 27")
    );
    assert!(kernel(&mut service, &mut env, "cmd undo").starts_with("ok undone"));
    assert!(kernel(&mut service, &mut env, "cmd help").contains("s |old|new|"));
    assert!(kernel(&mut service, &mut env, "cmd open Cargo.toml").contains(TOML));
    assert_eq!(
        kernel(&mut service, &mut env, "cmd reset"),
        "ok back to notes 2 as opened"
    );
    assert_eq!(
        kernel(&mut service, &mut env, "cmd diff"),
        "ok no changes since notes 2"
    );
}

#[test]
fn syncs_are_relayed_to_git_and_the_commit_comes_back() {
    let (mut service, mut env) = opened("x\n");
    let job = format!(
        "sync 3\ncomponent notes\nlabel e1\noutcome commit\ngeneration 1\nsource -\ncommit {COMMIT}\n"
    );
    assert_eq!(
        kernel(&mut service, &mut env, &job),
        format!("ok synced 3 {COMMIT}")
    );
    assert!(String::from_utf8_lossy(&env.requests[0]).starts_with("POST /sync "));
    env.host = Host::Forbidden;
    assert!(kernel(&mut service, &mut env, &job).starts_with("err build service: HTTP 403"));
}

#[test]
fn a_base_that_is_no_longer_stored_is_not_built() {
    let (mut service, mut env) = opened("x\n");
    kernel(&mut service, &mut env, "cmd c 1 y");
    env.artifacts.clear();
    let reply = kernel(&mut service, &mut env, "cmd build");
    assert!(reply.contains("is no longer stored"), "{reply}");
    assert!(env.requests.is_empty());
}

#[test]
fn huge_line_numbers_are_refused_not_overflowed() {
    let (mut service, mut env) = opened("x\n");
    for line in [
        "cmd p 18446744073709551615",
        "cmd a 18446744073709551615 x",
        "cmd p 4001,4002",
    ] {
        let reply = kernel(&mut service, &mut env, line);
        assert!(reply.starts_with("err "), "{line}: {reply}");
    }
}

#[test]
fn try_has_the_kernel_rehearse_the_last_build() {
    let (mut service, mut env) = opened("x\n");
    let nothing = "err nothing is built from this text; build first, then try";
    assert_eq!(kernel(&mut service, &mut env, "cmd try"), nothing);
    kernel(&mut service, &mut env, "cmd c 1 y");
    kernel(&mut service, &mut env, "cmd build");
    assert_eq!(
        kernel(&mut service, &mut env, "cmd try"),
        "ok try e1: import ok"
    );
    assert_eq!(
        env.controls,
        [(String::from("notes"), String::from("try e1"))]
    );
    env.control_reply = Ok("rubbish");
    assert_eq!(
        kernel(&mut service, &mut env, "cmd try"),
        "err notes answered \"rubbish\""
    );
    env.control_reply = Err(Error::NotFound);
    assert_eq!(
        kernel(&mut service, &mut env, "cmd try"),
        "err cannot reach notes: NotFound"
    );
    // An edit after the build leaves nothing built from the text.
    kernel(&mut service, &mut env, "cmd c 1 z");
    assert_eq!(kernel(&mut service, &mut env, "cmd try"), nothing);
    assert_eq!(env.controls.len(), 3);
}

#[test]
fn open_at_a_line_moves_the_cursor_there() {
    let (mut service, mut env) = opened("a\nb\nc\n");
    assert_eq!(
        kernel(&mut service, &mut env, &format!("open notes 2 at {LIB}:2")),
        format!("ok editing notes 2 at {LIB}:2")
    );
    assert!(kernel(&mut service, &mut env, "cmd p").contains("lines 2 to 3"));
    // A line past the end is the last line.
    assert_eq!(
        kernel(&mut service, &mut env, &format!("open notes 2 at {LIB}:9")),
        format!("ok editing notes 2 at {LIB}:3")
    );
    assert_eq!(
        kernel(
            &mut service,
            &mut env,
            "open notes 2 at components/notes/src/x.rs:1"
        ),
        "err components/notes/src/x.rs is not in notes 2"
    );
}

#[test]
fn open_at_a_line_never_drops_edits_that_are_not_built() {
    let (mut service, mut env) = opened("a\nb\n");
    env.artifacts
        .push(("e1", artifact("notes", "e1", &small("x\ny\nz\n"))));
    kernel(&mut service, &mut env, "cmd c 1 changed");
    let refused = kernel(&mut service, &mut env, &format!("open notes e1 at {LIB}:3"));
    assert!(
        refused.starts_with("err notes 2 has edits that are not built"),
        "{refused}"
    );
    assert!(service.session.as_ref().unwrap().changed());
    kernel(&mut service, &mut env, "cmd reset");
    assert_eq!(
        kernel(&mut service, &mut env, &format!("open notes e1 at {LIB}:3")),
        format!("ok editing notes e1 at {LIB}:3: 2 files")
    );
    assert_eq!(service.session.as_ref().unwrap().line(), 3);
}

#[test]
fn open_at_a_version_that_cannot_open_says_why_and_keeps_unbuilt_edits() {
    let (mut service, mut env) = opened("a\nb\n");
    let fixture = bundle(&[
        ("components/fixtures/notes-v1/Cargo.toml", b""),
        ("components/fixtures/notes-v1/src/lib.rs", b""),
    ]);
    env.artifacts.push(("1", artifact("notes", "1", &fixture)));
    kernel(&mut service, &mut env, "cmd c 1 changed");
    assert_eq!(
        kernel(&mut service, &mut env, &format!("open notes 1 at {LIB}:1")),
        "err notes 1 is not editable: its source is not components/notes"
    );
    assert!(
        kernel(&mut service, &mut env, &format!("open notes 7 at {LIB}:1"))
            .contains("no version notes 7")
    );
    let session = service.session.as_ref().unwrap();
    assert_eq!((session.label.as_str(), session.changed()), ("2", true));
    assert!(kernel(&mut service, &mut env, "cmd diff").contains("+ | changed"));
}

#[test]
fn open_at_a_line_in_an_edited_file_says_the_line_may_have_moved() {
    let (mut service, mut env) = opened("a\nb\nc\n");
    kernel(&mut service, &mut env, "cmd i 1 above");
    assert_eq!(
        kernel(&mut service, &mut env, &format!("open notes 2 at {LIB}:2")),
        format!(
            "ok editing notes 2 at {LIB}:2, with your edits; the line is 2's as stored, and your edits may have moved it"
        )
    );
    assert_eq!(service.session.as_ref().unwrap().line(), 2);
    // A file the edits left alone keeps the stored version's lines.
    assert_eq!(
        kernel(&mut service, &mut env, &format!("open notes 2 at {TOML}:1")),
        format!("ok editing notes 2 at {TOML}:1")
    );
}
