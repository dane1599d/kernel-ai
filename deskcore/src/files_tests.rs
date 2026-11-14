//! The file door's box: a write and a read from the first word to the last, who
//! may use it, how much, and how long.

use crate::files::{
    CHUNK, DOOR_APP, Files, IDLE_MS, MAX_BYTES, MAX_TRANSFERS, Op, SHELL, parse_job,
};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const PATH: &str = "/home/user/todo.txt";

/// A request as text, with a body after a newline; the answer's head line.
fn ask(files: &mut Files, caller: &str, text: &str, now: u64) -> String {
    let reply = files
        .request(caller, text.as_bytes(), now)
        .expect("a file request");
    String::from_utf8(reply.bytes).expect("text")
}

fn app(files: &mut Files, text: &str) -> String {
    ask(files, DOOR_APP, text, 0)
}

fn shell(files: &mut Files, text: &str) -> String {
    ask(files, SHELL, text, 0)
}

/// `n` bytes of text.
fn text(n: usize) -> String {
    (0..n).map(|i| (b'a' + (i % 26) as u8) as char).collect()
}

#[test]
fn a_write_goes_from_the_editor_to_the_shell_in_pieces_and_is_gone_when_told() {
    let mut files = Files::default();
    let body = text(CHUNK + 100);
    let total = body.len();
    assert_eq!(
        app(
            &mut files,
            &format!("files start 7 write old user {total} {PATH}")
        ),
        "ok"
    );
    // Not complete: the shell has nothing to take.
    assert_eq!(shell(&mut files, "files take"), "none");
    assert_eq!(
        app(&mut files, &format!("files data 7 0\n{}", &body[..CHUNK])),
        "ok"
    );
    assert_eq!(shell(&mut files, "files take"), "none");
    let last = files
        .request(
            DOOR_APP,
            format!("files data 7 {CHUNK}\n{}", &body[CHUNK..]).as_bytes(),
            0,
        )
        .unwrap();
    assert!(last.wake, "the last piece wakes the shell");
    let job = shell(&mut files, "files take");
    assert_eq!(job, format!("job 0 editor write old user {total} {PATH}"));
    assert_eq!(shell(&mut files, "files take"), "none", "taken once");
    assert_eq!(app(&mut files, "files poll 7"), "pending");
    // The shell fetches the pieces in order.
    let first = shell(&mut files, "files fetch 0 0");
    assert_eq!(first, format!("ok\n{}", &body[..CHUNK]));
    let second = shell(&mut files, &format!("files fetch 0 {CHUNK}"));
    assert_eq!(second, format!("ok\n{}", &body[CHUNK..]));
    assert_eq!(shell(&mut files, "files done 0 ok"), "ok");
    assert_eq!(app(&mut files, "files poll 7"), "ok");
    assert_eq!(files.running(), 0);
    assert_eq!(app(&mut files, "files poll 7"), "err that transfer is gone");
}

#[test]
fn a_read_goes_from_the_shell_to_the_editor_in_pieces() {
    let mut files = Files::default();
    let body = text(2 * CHUNK + 5);
    let start = files
        .request(DOOR_APP, format!("files start 1 read {PATH}").as_bytes(), 0)
        .unwrap();
    assert!(start.wake, "a read is complete at once");
    assert_eq!(
        shell(&mut files, "files take"),
        format!("job 0 editor read {PATH}")
    );
    let total = body.len();
    for (i, piece) in body.as_bytes().chunks(CHUNK).enumerate() {
        let piece = String::from_utf8_lossy(piece);
        let reply = shell(
            &mut files,
            &format!("files fill 0 {} {total}\n{piece}", i * CHUNK),
        );
        assert_eq!(reply, "ok");
    }
    assert_eq!(app(&mut files, "files poll 1"), "pending");
    assert_eq!(shell(&mut files, "files done 0 ok"), "ok");
    assert_eq!(app(&mut files, "files poll 1"), format!("ok {total}"));
    let mut got = String::new();
    for offset in (0..total).step_by(CHUNK) {
        let reply = app(&mut files, &format!("files get 1 {offset}"));
        let (head, piece) = reply.split_once('\n').unwrap();
        assert_eq!(head, format!("ok {total}"));
        got.push_str(piece);
    }
    assert_eq!(got, body);
    assert_eq!(files.running(), 0, "gone with its last piece");
}

/// #890: a transfer the shell took and gives back waits again, for the next
/// take; one it has begun to answer, or has not taken, is not its to give.
#[test]
fn the_shell_gives_back_a_transfer_it_has_not_begun_and_it_waits_again() {
    let mut files = Files::default();
    app(&mut files, &format!("files start 1 read {PATH}"));
    assert_eq!(
        shell(&mut files, "files back 0"),
        "err that transfer is not one the shell may give back",
        "not taken yet"
    );
    let job = shell(&mut files, "files take");
    assert_eq!(job, format!("job 0 editor read {PATH}"));
    assert_eq!(shell(&mut files, "files back 0"), "ok");
    assert_eq!(app(&mut files, "files poll 1"), "pending");
    assert_eq!(shell(&mut files, "files take"), job, "it waits again");
    assert_eq!(shell(&mut files, "files fill 0 0 2\nhi"), "ok");
    assert_eq!(
        shell(&mut files, "files back 0"),
        "err that transfer is not one the shell may give back",
        "begun"
    );
    assert_eq!(
        shell(&mut files, "files back 9"),
        "err that transfer is gone"
    );
    assert_eq!(app(&mut files, "files back 0"), "err not a file request");
}

/// #913's review: a write the shell has fetched a piece of may have been
/// written, so it is not the shell's to give back.
#[test]
fn a_write_fetched_is_not_given_back() {
    let mut files = Files::default();
    app(
        &mut files,
        &format!("files start 2 write old user 3 {PATH}"),
    );
    app(&mut files, "files data 2 0\nabc");
    shell(&mut files, "files take");
    assert_eq!(shell(&mut files, "files fetch 0 0"), "ok\nabc");
    assert_eq!(
        shell(&mut files, "files back 0"),
        "err that transfer is not one the shell may give back"
    );
}

/// `files take but <app>` passes by a transfer the shell holds back.
#[test]
fn a_take_may_skip_an_apps_transfer() {
    let mut files = Files::with_apps(&[DOOR_APP, "notes"]);
    app(&mut files, &format!("files start 1 read {PATH}"));
    ask(
        &mut files,
        "notes",
        &format!("files start 4 read {PATH}"),
        0,
    );
    assert_eq!(
        shell(&mut files, "files take but editor"),
        format!("job 1 notes read {PATH}")
    );
    assert_eq!(shell(&mut files, "files take but editor notes"), "none");
    assert_eq!(
        shell(&mut files, "files take"),
        format!("job 0 editor read {PATH}")
    );
}

/// A write keeps its bytes when given back, so the next take writes them all.
#[test]
fn a_write_given_back_keeps_its_bytes() {
    let mut files = Files::default();
    app(
        &mut files,
        &format!("files start 2 write old user 3 {PATH}"),
    );
    app(&mut files, "files data 2 0\nabc");
    shell(&mut files, "files take");
    assert_eq!(shell(&mut files, "files back 0"), "ok");
    assert_eq!(
        shell(&mut files, "files take"),
        format!("job 0 editor write old user 3 {PATH}")
    );
    // Taken again: not fetched yet in this take.
    assert_eq!(shell(&mut files, "files fetch 0 0"), "ok\nabc");
}

#[test]
fn an_empty_write_and_an_empty_read_need_no_pieces() {
    let mut files = Files::default();
    let started = files
        .request(
            DOOR_APP,
            format!("files start 1 write new - 0 {PATH}").as_bytes(),
            0,
        )
        .unwrap();
    assert!(started.wake);
    assert_eq!(
        shell(&mut files, "files take"),
        format!("job 0 editor write new - 0 {PATH}")
    );
    assert_eq!(shell(&mut files, "files done 0 ok"), "ok");
    assert_eq!(app(&mut files, "files poll 1"), "ok");
    app(&mut files, &format!("files start 2 read {PATH}"));
    shell(&mut files, "files take");
    shell(&mut files, "files done 1 ok");
    assert_eq!(app(&mut files, "files poll 2"), "ok 0");
    assert_eq!(app(&mut files, "files get 2 0"), "ok 0\n");
}

#[test]
fn the_shell_s_refusal_reaches_the_editor_as_its_reason() {
    let mut files = Files::default();
    app(
        &mut files,
        &format!("files start 1 write old user 3 {PATH}"),
    );
    app(&mut files, "files data 1 0\nabc");
    shell(&mut files, "files take");
    assert_eq!(
        shell(
            &mut files,
            "files done 0 err fs_write asks first: ask the AI"
        ),
        "ok"
    );
    assert_eq!(
        app(&mut files, "files poll 1"),
        "err fs_write asks first: ask the AI"
    );
    assert_eq!(files.running(), 0);
    // A read the shell says it could not do.
    app(&mut files, &format!("files start 2 read {PATH}"));
    shell(&mut files, "files take");
    shell(&mut files, "files done 1 err missing: no such file");
    assert_eq!(app(&mut files, "files poll 2"), "err missing: no such file");
}

#[test]
fn a_read_that_is_not_all_there_is_an_error() {
    let mut files = Files::default();
    app(&mut files, &format!("files start 1 read {PATH}"));
    shell(&mut files, "files take");
    shell(&mut files, "files fill 0 0 10\nabc");
    shell(&mut files, "files done 0 ok");
    assert_eq!(
        app(&mut files, "files poll 1"),
        "err the file was not all there"
    );
}

#[test]
fn only_the_editor_and_the_shell_use_the_door_and_each_its_own_half() {
    let mut files = Files::default();
    let refused = ask(&mut files, "notes", "files take", 0);
    assert!(refused.starts_with("err only the editor and the shell"));
    let refused = ask(
        &mut files,
        "pid 9",
        &format!("files start 1 read {PATH}"),
        0,
    );
    assert!(refused.starts_with("err only the editor and the shell"));
    // The editor cannot take the shell's half, nor the shell the editor's.
    assert_eq!(app(&mut files, "files take"), "err not a file request");
    assert_eq!(
        shell(&mut files, &format!("files start 1 read {PATH}")),
        "err not a file request"
    );
    // Not a file message at all.
    assert!(files.request(DOOR_APP, b"effects put\n", 0).is_none());
    assert!(files.request(DOOR_APP, b"filesystem", 0).is_none());
}

#[test]
fn a_bad_path_or_size_is_refused_before_anything_is_kept() {
    let mut files = Files::default();
    for path in [
        "todo.txt",
        "/home/../etc",
        "/home/user/a:b",
        "/home//x",
        "/",
        "/system/log",
        "/home/other/x",
        "/home/user",
        "/home/user/../../system/log",
    ] {
        let reply = app(&mut files, &format!("files start 1 read {path}"));
        assert!(reply.starts_with("err "), "{path}: {reply}");
    }
    let long = format!("/{}", "x".repeat(300));
    assert!(app(&mut files, &format!("files start 1 read {long}")).starts_with("err "));
    let big = MAX_BYTES + 1;
    let reply = app(
        &mut files,
        &format!("files start 1 write old user {big} {PATH}"),
    );
    assert!(reply.starts_with("err a file is at most"), "{reply}");
    assert!(app(&mut files, "files start 1 write maybe user 3 /a").starts_with("err "));
    assert_eq!(files.running(), 0);
    // The largest file is accepted.
    assert_eq!(
        app(
            &mut files,
            &format!("files start 2 write old user {MAX_BYTES} {PATH}")
        ),
        "ok"
    );
}

#[test]
fn pieces_out_of_order_or_over_what_was_said_drop_the_transfer() {
    for piece in [
        "files data 1 3\nabc",
        "files data 1 0\nabcdef",
        "files data 1 0\nab",
    ] {
        let mut files = Files::default();
        app(
            &mut files,
            &format!("files start 1 write old user 5 {PATH}"),
        );
        let reply = app(&mut files, piece);
        if piece.ends_with("ab") {
            // Short but in order: the rest may follow.
            assert_eq!(reply, "ok");
            assert_eq!(app(&mut files, "files data 1 2\ncde"), "ok");
            assert_eq!(files.running(), 1);
            continue;
        }
        assert!(reply.starts_with("err a piece out of order"), "{reply}");
        assert_eq!(files.running(), 0, "dropped: {piece}");
    }
    // A piece over a chunk.
    let mut files = Files::default();
    let total = CHUNK * 2;
    app(
        &mut files,
        &format!("files start 1 write old user {total} {PATH}"),
    );
    let reply = app(&mut files, &format!("files data 1 0\n{}", text(CHUNK + 1)));
    assert!(reply.starts_with("err "), "{reply}");
}

#[test]
fn the_door_is_busy_past_two_transfers_and_a_new_start_replaces_the_apps_old_one() {
    let mut files = Files::with_apps(&["editor", "files", "notes"]);
    assert_eq!(MAX_TRANSFERS, 2);
    assert_eq!(
        ask(
            &mut files,
            "editor",
            &format!("files start 1 read {PATH}"),
            0
        ),
        "ok"
    );
    assert_eq!(
        ask(
            &mut files,
            "files",
            &format!("files start 1 read {PATH}"),
            0
        ),
        "ok"
    );
    let busy = ask(
        &mut files,
        "notes",
        &format!("files start 1 read {PATH}"),
        0,
    );
    assert!(busy.starts_with("err busy"), "{busy}");
    assert_eq!(files.running(), 2);
    // The editor starting again drops its old transfer, so there is room.
    assert_eq!(
        ask(
            &mut files,
            "editor",
            &format!("files start 2 read {PATH}"),
            0
        ),
        "ok"
    );
    assert_eq!(files.running(), 2);
    assert_eq!(
        ask(&mut files, "editor", "files poll 1", 0),
        "err that transfer is gone"
    );
    // Done and fetched, a transfer's room is free for another app.
    let job = ask(&mut files, SHELL, "files take", 0);
    assert!(job.starts_with("job "));
    ask(&mut files, SHELL, "files done 1 err no", 0);
    ask(&mut files, "files", "files poll 1", 0);
    assert_eq!(
        ask(
            &mut files,
            "notes",
            &format!("files start 1 read {PATH}"),
            0
        ),
        "ok"
    );
}

#[test]
fn a_transfer_nothing_touches_is_dropped_after_the_idle_time() {
    let mut files = Files::default();
    ask(
        &mut files,
        DOOR_APP,
        &format!("files start 1 write old user 3 {PATH}"),
        1_000,
    );
    ask(
        &mut files,
        DOOR_APP,
        "files data 1 0\nab",
        1_000 + IDLE_MS - 1,
    );
    assert_eq!(files.running(), 1, "a touch keeps it");
    let gone = ask(&mut files, DOOR_APP, "files poll 1", 1_000 + 2 * IDLE_MS);
    assert_eq!(gone, "err that transfer is gone");
    assert_eq!(files.running(), 0);
    // A shell that took a job and died does not hold the door forever.
    ask(
        &mut files,
        DOOR_APP,
        &format!("files start 2 read {PATH}"),
        5_000_000,
    );
    ask(&mut files, SHELL, "files take", 5_000_000);
    files.expire(5_000_000 + IDLE_MS);
    assert_eq!(files.running(), 0);
}

#[test]
fn a_user_mark_travels_with_a_write_and_a_read_has_none() {
    let mut files = Files::default();
    app(&mut files, &format!("files start 1 write old - 1 {PATH}"));
    app(&mut files, "files data 1 0\nx");
    assert_eq!(
        shell(&mut files, "files take"),
        format!("job 0 editor write old - 1 {PATH}")
    );
    let mut bytes: Vec<u8> = Vec::new();
    bytes.extend_from_slice(shell(&mut files, "files fetch 0 0").as_bytes());
    assert_eq!(bytes, b"ok\nx");
}

#[test]
fn a_job_line_reads_back_as_the_job_the_shell_does() {
    let mut files = Files::default();
    app(
        &mut files,
        &format!("files start 3 write new user 4 {PATH}"),
    );
    app(&mut files, "files data 3 0\nabcd");
    let write = parse_job(&shell(&mut files, "files take")).unwrap();
    assert_eq!(
        (
            write.app.as_str(),
            write.op,
            write.user,
            write.total,
            write.path.as_str()
        ),
        ("editor", Op::Write { create: true }, true, 4, PATH)
    );
    app(&mut files, &format!("files start 4 read {PATH}"));
    let read = parse_job(&shell(&mut files, "files take")).unwrap();
    assert_eq!((read.op, read.user, read.total), (Op::Read, false, 0));
    assert_eq!(parse_job("none"), None);
    assert_eq!(parse_job("job x editor read /a"), None);
    assert_eq!(parse_job("job 1 editor write maybe user 3 /a"), None);
    assert_eq!(parse_job("err busy"), None);
}

#[test]
fn a_build_goes_from_the_editor_to_the_shell_which_answers_with_a_report() {
    let mut files = Files::default();
    let started = files
        .request(DOOR_APP, b"files start 9 build user hello", 0)
        .unwrap();
    assert!(
        started.wake,
        "a build needs no pieces: the shell wakes at once"
    );
    assert_eq!(
        shell(&mut files, "files take"),
        "job 0 editor build user hello"
    );
    assert_eq!(shell(&mut files, "files take"), "none", "taken once");
    assert_eq!(app(&mut files, "files poll 9"), "pending");
    // A build carries nothing in, so there is nothing for the shell to fetch.
    assert_eq!(
        shell(&mut files, "files fetch 0 0"),
        "err there is nothing to fetch"
    );
    // The report is several pieces, as many lines as the compiler said.
    let report = format!("err cc build hello: hello.c:3: error\n{}", text(CHUNK + 40));
    let total = report.len();
    for (i, piece) in report.as_bytes().chunks(CHUNK).enumerate() {
        let piece = String::from_utf8_lossy(piece);
        let sent = shell(
            &mut files,
            &format!("files fill 0 {} {total}\n{piece}", i * CHUNK),
        );
        assert_eq!(sent, "ok");
    }
    assert_eq!(shell(&mut files, "files done 0 ok"), "ok");
    assert_eq!(app(&mut files, "files poll 9"), format!("ok {total}"));
    let mut got = String::new();
    for offset in (0..total).step_by(CHUNK) {
        let reply = app(&mut files, &format!("files get 9 {offset}"));
        let (head, piece) = reply.split_once('\n').unwrap();
        assert_eq!(head, format!("ok {total}"));
        got.push_str(piece);
    }
    assert_eq!(got, report);
    assert_eq!(files.running(), 0, "gone with its last piece");
}

#[test]
fn a_build_the_shell_refuses_reaches_the_editor_as_the_reason() {
    let mut files = Files::default();
    app(&mut files, "files start 1 build - hello");
    assert_eq!(
        shell(&mut files, "files take"),
        "job 0 editor build - hello"
    );
    shell(
        &mut files,
        "files done 0 err a build needs a key or click of the person at the editor",
    );
    assert_eq!(
        app(&mut files, "files poll 1"),
        "err a build needs a key or click of the person at the editor"
    );
    assert_eq!(files.running(), 0);
}

#[test]
fn a_build_that_says_it_is_done_without_all_its_report_is_an_error() {
    let mut files = Files::default();
    app(&mut files, "files start 1 build user hello");
    shell(&mut files, "files take");
    shell(&mut files, "files fill 0 0 10\nok built");
    shell(&mut files, "files done 0 ok");
    assert_eq!(
        app(&mut files, "files poll 1"),
        "err the file was not all there"
    );
}

#[test]
fn only_a_component_s_name_is_a_build() {
    let mut files = Files::default();
    for name in [
        "../x",
        "a/b",
        "Hello",
        "h",
        "hello-world-12",
        "x y",
        "1abc",
        "a:b",
        "-ab",
        "héllo",
        "/home/user/c/hello",
    ] {
        let reply = app(&mut files, &format!("files start 1 build user {name}"));
        assert!(reply.starts_with("err "), "{name}: {reply}");
    }
    // Not a build's words: the user mark, or a missing name.
    for text in [
        "files start 1 build hello",
        "files start 1 build maybe hello",
        "files start 1 build user",
        "files start 1 build user hello extra",
    ] {
        assert_eq!(app(&mut files, text), "err not a file request", "{text}");
    }
    assert_eq!(files.running(), 0);
    for name in ["hello", "h2", "my-app-1", "abcdefghijkl"] {
        assert_eq!(
            app(&mut files, &format!("files start 1 build user {name}")),
            "ok",
            "{name}"
        );
    }
}

#[test]
fn a_build_holds_one_of_the_two_places_and_only_the_editor_may_ask() {
    let mut files = Files::with_apps(&["editor", "files"]);
    assert_eq!(
        ask(&mut files, "editor", "files start 1 build user hello", 0),
        "ok"
    );
    assert_eq!(
        ask(
            &mut files,
            "files",
            &format!("files start 1 read {PATH}"),
            0
        ),
        "ok"
    );
    let busy = ask(&mut files, "editor", "files start 2 read /home/user/a", 0);
    // The editor's new start replaces its old build, so it is not busy.
    assert_eq!(busy, "ok");
    assert_eq!(files.running(), 2);
    let refused = ask(&mut files, "notes", "files start 1 build user hello", 0);
    assert!(refused.starts_with("err only the editor and the shell"));
    assert_eq!(
        shell(&mut files, "files start 1 build user hello"),
        "err not a file request",
        "the shell cannot start one"
    );
}

#[test]
fn a_build_job_line_reads_back_as_the_job_the_shell_does() {
    let mut files = Files::default();
    app(&mut files, "files start 3 build user hello");
    let job = parse_job(&shell(&mut files, "files take")).unwrap();
    assert_eq!(
        (
            job.app.as_str(),
            job.op,
            job.user,
            job.total,
            job.path.as_str()
        ),
        ("editor", Op::Build, true, 0, "hello")
    );
    app(&mut files, "files start 4 build - hello");
    let job = parse_job(&shell(&mut files, "files take")).unwrap();
    assert_eq!((job.op, job.user), (Op::Build, false));
    assert_eq!(parse_job("job 1 editor build maybe hello"), None);
    assert_eq!(parse_job("job 1 editor build user"), None);
}
