use super::*;
use crate::session::{Session, UNDO};
use livecore::artifact::source_files;

fn open(lib: &str) -> Session {
    Session::open("notes", "2", &small(lib)).unwrap()
}

fn lib_text(session: &Session) -> String {
    let bundle = session.to_bundle();
    let files = source_files(&bundle).unwrap();
    let (_, bytes) = files.iter().find(|(p, _)| *p == LIB).unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[test]
fn the_real_notes_source_opens_on_lib_rs() {
    let bundle = real_notes();
    let session = Session::open("notes", "2", &bundle).unwrap();
    assert_eq!(session.files().len(), 5);
    assert_eq!(session.current().path, LIB);
    assert!(!session.changed());
    assert_eq!(session.to_bundle(), bundle);
}

#[test]
fn files_resolve_by_path_or_a_suffix_only_one_has() {
    let bundle = bundle(&[
        ("components/notes/src/a/x.rs", b"a\n"),
        ("components/notes/src/b/x.rs", b"b\n"),
        (LIB, b"lib\n"),
    ]);
    let mut session = Session::open("notes", "2", &bundle).unwrap();
    assert_eq!(session.resolve(LIB), Ok(2));
    assert_eq!(session.resolve("lib.rs"), Ok(2));
    assert_eq!(session.resolve("src/lib.rs"), Ok(2));
    assert_eq!(session.resolve("a/x.rs"), Ok(0));
    assert!(
        session
            .resolve("x.rs")
            .unwrap_err()
            .contains("names 2 files")
    );
    assert!(session.resolve("ib.rs").unwrap_err().contains("no file"));
    session.select("b/x.rs").unwrap();
    assert_eq!(session.current().path, "components/notes/src/b/x.rs");
}

#[test]
fn edits_check_their_line_numbers() {
    let mut session = open(&numbered(3, "\n"));
    assert!(session.change(0, "x").is_err());
    assert!(session.change(4, "x").is_err());
    assert!(session.insert(5, "x").is_err());
    assert!(session.delete(2, 1).is_err());
    assert!(session.delete(1, 4).is_err());
    assert!(session.change(1, "two\nlines").is_err());
    session.change(2, "two").unwrap();
    session.insert(4, "four").unwrap();
    session.insert(1, "zero").unwrap();
    assert_eq!(lib_text(&session), "zero\nline 1\ntwo\nline 3\nfour\n");
    session.delete(2, 3).unwrap();
    assert_eq!(lib_text(&session), "zero\nline 3\nfour\n");
    assert_eq!(session.line(), 2);
}

#[test]
fn substitute_needs_exactly_one_match() {
    let mut session = open("let a = 1;\nlet b = 1;\nlet c = 2;\n");
    assert!(
        session
            .substitute(None, "= 3", "x")
            .unwrap_err()
            .contains("is not in")
    );
    assert!(
        session
            .substitute(None, "= 1", "= 9")
            .unwrap_err()
            .contains("2 times")
    );
    assert_eq!(session.substitute(Some(2), "= 1", "= 9"), Ok(2));
    assert_eq!(session.substitute(None, "c = 2", "c = 3"), Ok(3));
    assert!(session.substitute(Some(1), "b", "x").is_err());
    assert!(session.substitute(None, "", "x").is_err());
    assert_eq!(lib_text(&session), "let a = 1;\nlet b = 9;\nlet c = 3;\n");
}

#[test]
fn crlf_and_a_missing_final_newline_round_trip_byte_for_byte() {
    let text = "line 1\r\nline 2\r\nline 3";
    let mut session = open(text);
    assert_eq!(lib_text(&session), text);
    assert!(!session.changed());
    session.change(2, "two").unwrap();
    assert_eq!(lib_text(&session), "line 1\r\ntwo\r\nline 3");
    // New lines take the file's ending; the last still has no newline.
    session.insert(4, "four").unwrap();
    assert_eq!(lib_text(&session), "line 1\r\ntwo\r\nline 3\r\nfour");
    session.insert(1, "zero").unwrap();
    assert!(lib_text(&session).starts_with("zero\r\nline 1\r\n"));
    session.delete(5, 5).unwrap();
    assert_eq!(lib_text(&session), "zero\r\nline 1\r\ntwo\r\nline 3");
    // Mixed endings stay as they were.
    let mixed = "a\nb\r\nc\n";
    let mut session = open(mixed);
    session.change(3, "C").unwrap();
    assert_eq!(lib_text(&session), "a\nb\r\nC\n");
}

#[test]
fn undo_takes_back_up_to_32_edits_and_reset_drops_them_all() {
    let original = numbered(3, "\n");
    let mut session = open(&original);
    for n in 0..UNDO + 3 {
        session.change(1, &format!("edit {n}")).unwrap();
    }
    for _ in 0..UNDO {
        assert!(session.undo());
    }
    assert!(!session.undo());
    assert_eq!(lib_text(&session), "edit 2\nline 2\nline 3\n");
    session.reset();
    assert_eq!(lib_text(&session), original);
    assert!(!session.changed());
}

#[test]
fn the_bundle_is_the_edited_files_and_any_edit_clears_built() {
    let mut session = open("a\nb\n");
    session.substitute(None, "b", "B").unwrap();
    assert_eq!(
        session.to_bundle(),
        bundle(&[
            (TOML, b"[package]\nname = \"comp-notes\"\n"),
            (LIB, b"a\nB\n")
        ])
    );
    assert!(session.changed());
    assert!(session.file_changed(1));
    assert!(!session.file_changed(0));
    session.built = Some("e1".into());
    session.insert(1, "x").unwrap();
    assert_eq!(session.built, None);
    session.built = Some("e1".into());
    session.undo();
    assert_eq!(session.built, None);
}

#[test]
fn find_lists_every_line_and_goto_moves_there() {
    let mut session = open("x\nfoo\nbar foo\n");
    assert_eq!(session.find("foo"), [(1, 2), (1, 3)]);
    assert!(session.goto(LIB, 3));
    assert_eq!(session.line(), 3);
    assert!(!session.goto("nope.rs", 1));
}

#[test]
fn bundles_that_are_too_big_or_not_text_are_refused() {
    let big = "x".repeat(crate::MAX_BUNDLE);
    assert!(Session::open("notes", "2", &small(&big)).is_err());
    let binary = bundle(&[(LIB, b"\xff\xfe")]);
    assert!(
        Session::open("notes", "2", &binary)
            .unwrap_err()
            .contains("not text")
    );
    assert!(Session::open("notes", "2", b"junk").is_err());
}

#[test]
fn overlapping_matches_count_as_more_than_one() {
    let mut session = open("x = 1000;\n");
    assert!(
        session
            .substitute(None, "00", "01")
            .unwrap_err()
            .contains("2 times")
    );
    assert_eq!(lib_text(&session), "x = 1000;\n");
}

#[test]
fn an_edit_cannot_grow_the_source_past_what_a_build_takes() {
    let mut session = open("a\nb\n");
    let room = crate::MAX_BUNDLE;
    let big = "x".repeat(room);
    assert!(session.change(1, &big).unwrap_err().contains("would pass"));
    assert!(session.insert(1, &big).is_err());
    assert!(session.substitute(None, "a", &big).is_err());
    assert!(!session.changed());
    // Fill most of the room, then see that the next line that does not fit is
    // refused while a smaller one still goes in.
    session.change(1, &"y".repeat(room - 2_000)).unwrap();
    assert!(session.insert(2, &"z".repeat(3_000)).is_err());
    session.insert(2, "small").unwrap();
    assert!(session.undo());
    assert!(session.undo());
    assert!(!session.changed());
}
