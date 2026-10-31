//! The file door's box: a write and a read from the first word to the last, who
//! may use it, how much, and how long.

use crate::files::{CHUNK, DOOR_APP, Files, IDLE_MS, MAX_BYTES, MAX_TRANSFERS, SHELL};
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
