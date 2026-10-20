use super::LIB;
use super::fake::{Fake, Host, Job, TOKEN, build_now, kernel, opened, status_until_done};
use crate::service::Service;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::Error;

#[test]
fn status_says_what_is_open_and_how_the_last_build_went() {
    let mut service = Service::new("10.0.2.2:5000", TOKEN);
    let mut env = Fake::new("x\n");
    assert_eq!(
        kernel(&mut service, &mut env, "cmd status"),
        "ok no file is open"
    );
    let (mut service, mut env) = opened("x\n");
    assert_eq!(
        kernel(&mut service, &mut env, "cmd status"),
        "ok notes 2: unchanged"
    );
    kernel(&mut service, &mut env, "cmd c 1 y");
    assert_eq!(
        kernel(&mut service, &mut env, "cmd status"),
        "ok notes 2: edited"
    );
    kernel(&mut service, &mut env, "cmd build");
    assert_eq!(
        kernel(&mut service, &mut env, "cmd status"),
        "ok notes 2: edited, built e1\nlast build: ok built e1 from 2 (commit 1a2b3c4d5e6f); :apply runs it"
    );
}

#[test]
fn the_moved_mark_splits_only_a_well_formed_first_line() {
    use crate::ipc::{split_moved, with_moved};
    let marked = with_moved("components/notes/src/lib.rs:4", "ok p\nmore");
    assert_eq!(
        split_moved(marked.as_bytes()),
        (Some("components/notes/src/lib.rs:4"), &b"ok p\nmore"[..])
    );
    for whole in [
        &b"ok plain"[..],
        b"#at lib.rs:4",
        b"#at \xff\xfe:4\nok",
        b"ok p\n#at lib.rs:4",
    ] {
        assert_eq!(split_moved(whole), (None, whole));
    }
}

const SYNC: &str = "sync 3\ncomponent notes\nlabel e1\noutcome commit\ngeneration 1\nsource -\ncommit 1a2b3c4d5e6f0123456789abcdef0123456789ab\n";
const BUILT: &str = "ok built e1 from 2 (commit 1a2b3c4d5e6f); :apply runs it";

#[test]
fn a_build_answers_at_once_and_its_result_follows_the_polls() {
    let (mut service, mut env) = opened("x\n");
    kernel(&mut service, &mut env, "cmd c 1 y");
    env.busy_polls = 2;
    assert_eq!(
        kernel(&mut service, &mut env, "cmd build"),
        "ok building e1 from 2"
    );
    assert!(env.puts.is_empty() && service.polling());
    for _ in 0..2 {
        assert_eq!(
            kernel(&mut service, &mut env, "cmd status"),
            "ok notes 2: edited; building e1 from 2"
        );
    }
    assert_eq!(
        kernel(&mut service, &mut env, "cmd status"),
        format!("ok notes 2: edited, built e1\nlast build: {BUILT}")
    );
    let (component, _, base, _) = &env.puts[0];
    assert_eq!(
        (component.as_str(), base.as_str(), env.puts.len()),
        ("notes", "2", 1)
    );
    assert_eq!(env.logs, [&BUILT[3..]]);
    assert!(!service.polling());
}

#[test]
fn edits_p_diff_and_status_answer_while_building() {
    let (mut service, mut env) = opened("x\ny\n");
    kernel(&mut service, &mut env, "cmd c 1 z");
    env.busy_polls = 4;
    kernel(&mut service, &mut env, "cmd build");
    assert!(kernel(&mut service, &mut env, "cmd c 2 w").starts_with("ok "));
    assert!(kernel(&mut service, &mut env, "cmd p").starts_with("ok "));
    assert!(kernel(&mut service, &mut env, "cmd diff").contains("+ | w"));
    let status = status_until_done(&mut service, &mut env);
    assert!(
        status.ends_with(&format!("{BUILT}; your edits since are not in it")),
        "{status}"
    );
    // Stored all the same, but not built from this text.
    assert_eq!(env.puts.len(), 1);
    assert_eq!(service.session.as_ref().unwrap().built, None);
}

#[test]
fn a_second_build_and_a_sync_while_building_are_busy() {
    let (mut service, mut env) = opened("x\n");
    kernel(&mut service, &mut env, "cmd c 1 y");
    env.busy_polls = 3;
    kernel(&mut service, &mut env, "cmd build");
    assert_eq!(
        kernel(&mut service, &mut env, "cmd build"),
        "err busy building e1"
    );
    assert_eq!(kernel(&mut service, &mut env, SYNC), "err busy building e1");
    assert_eq!(env.requests.len(), 1);
    status_until_done(&mut service, &mut env);
    assert!(kernel(&mut service, &mut env, SYNC).starts_with("ok synced 3 "));
    assert_eq!(env.requests.len(), 2);
}

#[test]
fn a_failed_background_build_moves_the_cursor_only_on_unchanged_text() {
    let (mut service, mut env) = opened("a\nb\nc\n");
    kernel(&mut service, &mut env, "cmd c 3 changed");
    env.host = Host::Fails;
    kernel(&mut service, &mut env, "cmd build");
    let moved = kernel(&mut service, &mut env, "cmd status");
    assert!(
        moved.starts_with(&format!(
            "#at {LIB}:2\nok notes 2: edited\nlast build: err build failed; the cursor is on the first error\n"
        )),
        "{moved}"
    );
    assert_eq!(service.session.as_ref().unwrap().line(), 2);
    // Edited while it builds: the answer says so, and the cursor stays.
    env.busy_polls = 1;
    kernel(&mut service, &mut env, "cmd build");
    kernel(&mut service, &mut env, "cmd c 3 again");
    let kept = kernel(&mut service, &mut env, "cmd status");
    assert!(
        kept.starts_with("ok notes 2: edited\nlast build: err build failed\n"),
        "{kept}"
    );
    assert_eq!(service.session.as_ref().unwrap().line(), 3);
}

#[test]
fn a_cursor_moved_outside_a_command_rides_on_the_next_one() {
    let (mut service, mut env) = opened("a\nb\nc\n");
    kernel(&mut service, &mut env, "cmd c 3 changed");
    env.host = Host::Fails;
    kernel(&mut service, &mut env, "cmd build");
    // Between requests, as the idle poll does.
    service.poll(&mut env);
    assert!(!kernel(&mut service, &mut env, SYNC).starts_with("#at "));
    let reply = kernel(&mut service, &mut env, "cmd p 1");
    assert!(reply.starts_with(&format!("#at {LIB}:2\nok ")), "{reply}");
    assert!(!kernel(&mut service, &mut env, "cmd p 1").starts_with("#at "));
}

#[test]
fn a_lost_or_failed_exchange_says_so() {
    let (mut service, mut env) = opened("x\n");
    kernel(&mut service, &mut env, "cmd c 1 y");
    kernel(&mut service, &mut env, "cmd build");
    env.job = None;
    let lost = status_until_done(&mut service, &mut env);
    assert!(
        lost.ends_with("last build: err the build of e1 was lost; build again"),
        "{lost}"
    );
    env.exchange = Err(Error::TimedOut);
    let failed = build_now(&mut service, &mut env);
    assert_eq!(failed, "err build service: TimedOut");
    assert_eq!(
        env.logs.last().map(String::as_str),
        Some("build service: TimedOut")
    );
    assert!(env.puts.is_empty());
}

#[test]
fn a_new_build_clears_the_last_result() {
    let (mut service, mut env) = opened("x\n");
    kernel(&mut service, &mut env, "cmd c 1 y");
    assert_eq!(build_now(&mut service, &mut env), BUILT);
    kernel(&mut service, &mut env, "cmd c 1 z");
    env.busy_polls = 1;
    kernel(&mut service, &mut env, "cmd build");
    assert_eq!(
        kernel(&mut service, &mut env, "cmd status"),
        "ok notes 2: edited; building e1 from 2"
    );
}

#[test]
fn status_shows_a_build_running_with_no_file_open() {
    let mut service = Service::new("10.0.2.2:5000", TOKEN);
    let mut env = Fake::new("x\n");
    env.job = Some(Job {
        reply: Ok(Vec::new()),
        busy: 5,
    });
    assert_eq!(
        kernel(&mut service, &mut env, "cmd status"),
        "ok no file is open; building, started before the edit service restarted"
    );
    assert!(service.polling());
}

#[test]
fn a_poll_fault_keeps_the_job() {
    let (mut service, mut env) = opened("x\n");
    kernel(&mut service, &mut env, "cmd c 1 y");
    kernel(&mut service, &mut env, "cmd build");
    env.fault_polls = 2;
    for _ in 0..2 {
        let status = kernel(&mut service, &mut env, "cmd status");
        assert_eq!(status, "ok notes 2: edited; building e1 from 2");
    }
    let done = status_until_done(&mut service, &mut env);
    assert!(done.ends_with(BUILT), "{done}");
}
