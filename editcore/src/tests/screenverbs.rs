//! `split`, `join` and `view`: the verbs a full-screen editor needs.

use super::fake::{kernel, opened};
use super::*;
use crate::command::{Command, parse};
use crate::journal::Journal;
use crate::session::{MAX_INDENT, Session};

fn open(lib: &str) -> Session {
    Session::open("notes", "2", &small(lib)).unwrap()
}

fn text(session: &Session) -> String {
    String::from_utf8(session.current().bytes()).unwrap()
}

#[test]
fn the_new_commands_parse() {
    assert_eq!(
        parse("split 3 7"),
        Ok(Command::Split {
            line: 3,
            col: 7,
            indent: 0
        })
    );
    assert_eq!(
        parse("split 3 0 4"),
        Ok(Command::Split {
            line: 3,
            col: 0,
            indent: 4
        })
    );
    assert_eq!(parse("join 9"), Ok(Command::Join(9)));
    assert_eq!(parse("view 1,40"), Ok(Command::View(1, 40)));
    assert_eq!(parse("view 5"), Ok(Command::View(5, 5)));
    for bad in [
        "split",
        "split 3",
        "split 0 1",
        "split 3 x",
        "split 3 1 2 3",
        "split 3 99999999",
        "join",
        "join 0",
        "join 1,2",
        "view",
        "view 0,3",
        "view 1,2,3",
    ] {
        assert!(parse(bad).is_err(), "{bad:?} parsed");
    }
}

#[test]
fn a_split_breaks_a_line_and_one_undo_joins_it_again() {
    let mut session = open("alpha beta\nz\n");
    session.split(1, 5, 0).unwrap();
    assert_eq!(text(&session), "alpha\n beta\nz\n");
    assert_eq!(session.line(), 2);
    assert!(session.undo());
    assert_eq!(text(&session), "alpha beta\nz\n");
    session.split(1, 5, 4).unwrap();
    assert_eq!(text(&session), "alpha\n     beta\nz\n");
    session.undo();
    session.split(1, 0, 0).unwrap();
    assert_eq!(text(&session), "\nalpha beta\nz\n");
    session.undo();
    session.split(1, 10, 0).unwrap();
    assert_eq!(text(&session), "alpha beta\n\nz\n");
}

#[test]
fn a_split_counts_characters_not_bytes() {
    let mut session = open("h\u{e9}llo w\u{f6}rld\n");
    session.split(1, 2, 0).unwrap();
    assert_eq!(text(&session), "h\u{e9}\nllo w\u{f6}rld\n");
    assert!(session.split(2, 99, 0).is_err());
}

#[test]
fn a_split_of_the_last_line_keeps_the_missing_final_newline_at_the_end() {
    let mut session = open("one\ntwo");
    session.split(2, 1, 0).unwrap();
    assert_eq!(text(&session), "one\nt\nwo");
    session.undo();
    assert_eq!(text(&session), "one\ntwo");
    let mut crlf = open("one\r\ntwo");
    crlf.split(2, 1, 0).unwrap();
    assert_eq!(text(&crlf), "one\r\nt\r\nwo");
}

#[test]
fn a_split_refuses_what_it_cannot_do() {
    let mut session = open("a\nb\n");
    assert!(session.split(0, 0, 0).is_err());
    assert!(session.split(3, 0, 0).is_err());
    assert!(session.split(1, 2, 0).is_err());
    assert!(session.split(1, 0, MAX_INDENT + 1).is_err());
    assert!(!session.changed());
    let full = numbered(crate::MAX_LINES, "\n");
    let mut session = open(&full);
    assert!(session.split(1, 1, 0).unwrap_err().contains("at most"));
}

#[test]
fn a_join_puts_the_next_line_on_this_one_and_undo_splits_it() {
    let mut session = open("one\r\ntwo\r\nthree");
    session.join(1).unwrap();
    assert_eq!(text(&session), "onetwo\r\nthree");
    assert_eq!(session.line(), 1);
    session.undo();
    assert_eq!(text(&session), "one\r\ntwo\r\nthree");
    session.join(2).unwrap();
    assert_eq!(text(&session), "one\r\ntwothree");
    assert!(session.join(2).is_err());
    assert!(session.join(0).is_err());
}

#[test]
fn view_answers_lines_and_the_file_state_without_moving_the_cursor() {
    let (mut service, mut env) = opened(&numbered(8, "\n"));
    kernel(&mut service, &mut env, "cmd p 2,3");
    let before = kernel(&mut service, &mut env, "cmd view 2,4");
    assert_eq!(
        before,
        format!("ok view notes 2 {LIB} 8 4 -\n2:line 2\n3:line 3\n4:line 4")
    );
    assert_eq!(kernel(&mut service, &mut env, "cmd view 2,4"), before);
    kernel(&mut service, &mut env, "cmd c 3 changed");
    let reply = kernel(&mut service, &mut env, "cmd view 7,99");
    assert!(
        reply.starts_with(&format!("ok view notes 2 {LIB} 8 3 cu\n7:line 7\n8:line 8")),
        "{reply}"
    );
    assert!(
        kernel(&mut service, &mut env, "cmd view 9,10")
            .starts_with(&format!("ok view notes 2 {LIB} 8 3 cu"))
    );
}

#[test]
fn view_marks_lines_it_cut_or_cleaned_and_always_fits_a_message() {
    let long = "x".repeat(300);
    let lib = format!("tab\there\n{long}\nbell\u{7}\n");
    let (mut service, mut env) = opened(&lib);
    let reply = kernel(&mut service, &mut env, "cmd view 1,3");
    let lines: Vec<&str> = reply.lines().collect();
    assert_eq!(lines[1], "1:tab\there");
    assert_eq!(
        lines[2],
        format!("2~{}", "x".repeat(crate::service::view::MAX_VIEW_CHARS))
    );
    assert_eq!(lines[3], "3~bell?");

    let wide = (1..=200)
        .map(|n| format!("{n} {}\n", "y".repeat(190)))
        .collect::<String>();
    let (mut service, mut env) = opened(&wide);
    let reply = kernel(&mut service, &mut env, "cmd view 1,64");
    assert!(reply.len() < kabi::MAX_MESSAGE - 8 - crate::ipc::MARK_ROOM);
    let got = reply.lines().count() - 1;
    assert!((20..64).contains(&got), "{got} lines");
}

#[test]
fn a_service_edit_by_split_and_join_replays_into_a_fresh_service() {
    let mut service = super::fake::service();
    let mut env = super::fake::Fake::new("alpha beta\ngamma\n");
    let mut journal = Journal::new();
    for message in [
        "open notes",
        "cmd split 1 5 2",
        "cmd join 2",
        "cmd split 1 5",
        "cmd view 1,3",
    ] {
        let reply = service.handle(&mut env, 0, message.as_bytes());
        journal.record(message.as_bytes(), reply.as_bytes());
    }
    // The view was not journaled; the three edits and the open were.
    assert_eq!(journal.len(), 4);
    assert_eq!(journal.edits(), 3);
    let (mut fresh, restore) = super::fake::restore(&mut journal, &mut env);
    assert!(
        restore.lost.is_empty(),
        "{:?} {}",
        restore.lost,
        restore.notice
    );
    assert_eq!(
        kernel(&mut fresh, &mut env, "cmd view 1,3"),
        kernel(&mut service, &mut env, "cmd view 1,3")
    );
}
