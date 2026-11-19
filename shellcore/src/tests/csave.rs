//! `:save` and `:load` at `c>` with the real `cc` on the host (ADR 0012 D4):
//! a saved file starts `#include <kai.h>` and holds the live definitions; a
//! load replays definitions, never statements; both stay in
//! `/home/user/c/`; `undo` at `c>` reverts either.

use super::cmode::start_cc;
use super::*;

fn c_shell(host: &Rc<FakeHost>) -> Shell {
    start_cc(host);
    let mut shell = shell(host);
    assert!(shell.eval(":c").ok);
    shell
}

fn file(host: &FakeHost, path: &str) -> Option<String> {
    host.files.borrow().get(path).cloned()
}

#[test]
fn save_writes_the_definitions_and_load_replays_them_after_a_reset() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    assert!(shell.eval("I64 Sq(I64 x) { return x * x; }").ok);
    assert!(shell.eval("I64 g = 5;").ok);
    assert_eq!(shell.eval("Print(\"run once\\n\");").text, "run once");
    let saved = shell.eval(":save demo");
    assert_eq!(
        saved.text, "saved 2 entries to /home/user/c/demo.c; undo puts it back",
        "{saved:?}"
    );
    let text = file(&host, "c/demo.c").expect("the file is written");
    assert!(text.starts_with("#include <kai.h>\n"), "{text}");
    assert!(text.contains("I64 Sq(I64 x) { return x * x; }"), "{text}");
    assert!(text.contains("I64 g = 5;"), "{text}");
    assert!(
        !text.contains("run once"),
        "a statement-only entry is no definition"
    );
    assert_eq!(shell.eval(":reset").text, "the C session is empty again");
    assert!(!shell.eval("Sq(2)").ok);
    let loaded = shell.eval(":load demo");
    assert!(loaded.ok, "{loaded:?}");
    assert!(loaded.text.starts_with("loaded "), "{loaded:?}");
    assert!(loaded.text.contains("/home/user/c/demo.c"), "{loaded:?}");
    assert_eq!(shell.eval("Sq(3)").text, "= 9");
}

#[test]
fn a_load_replays_definitions_never_statements() {
    let host = FakeHost::new();
    host.files.borrow_mut().insert(
        "c/mixed.c".into(),
        "#include <kai.h>\nI64 Tw(I64 x) { return x * 2; }\nPrint(\"nope\\n\");\n".into(),
    );
    let mut shell = c_shell(&host);
    let loaded = shell.eval(":load mixed");
    assert!(loaded.ok && !loaded.text.contains("nope"), "{loaded:?}");
    assert_eq!(shell.eval("Tw(4)").text, "= 8");
}

#[test]
fn undo_at_c_takes_a_load_and_a_save_back() {
    let host = FakeHost::new();
    host.files
        .borrow_mut()
        .insert("c/lib.c".into(), "I64 One() { return 1; }\n".into());
    let mut shell = c_shell(&host);
    assert!(shell.eval(":load lib").ok);
    assert_eq!(shell.eval("One()").text, "= 1");
    assert!(shell.eval("undo").ok);
    assert!(!shell.eval("One()").ok, "its definitions are gone");
    // A save over a file that is there is undone back to its earlier text.
    assert!(shell.eval("I64 Two() { return 2; }").ok);
    assert!(shell.eval(":save lib").ok);
    assert!(file(&host, "c/lib.c").unwrap().contains("Two"));
    let undone = shell.eval("undo");
    assert!(undone.ok, "{undone:?}");
    assert_eq!(file(&host, "c/lib.c").unwrap(), "I64 One() { return 1; }\n");
}

#[test]
fn names_stay_in_the_c_folder() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    assert!(shell.eval("I64 One() { return 1; }").ok);
    for name in ["../notes", "a/b", "/etc/x", ".hidden", "x.c", "1st", ""] {
        let line = format!(":save {name}");
        let saved = shell.eval(&line);
        assert!(!saved.ok, "{line}: {saved:?}");
        let loaded = shell.eval(&format!(":load {name}"));
        assert!(!loaded.ok, "{name}: {loaded:?}");
    }
    assert!(host.files.borrow().is_empty(), "nothing written");
    let missing = shell.eval(":load nothere");
    assert_eq!(missing.text, "/home/user/c/nothere.c is not there");
    let empty = c_shell(&FakeHost::new()).eval(":save demo");
    assert!(
        !empty.ok && empty.text.contains("nothing is defined"),
        "{empty:?}"
    );
}

#[test]
fn an_include_at_the_top_reads_a_file_from_the_persons_folders() {
    let host = FakeHost::new();
    host.files.borrow_mut().insert(
        "c/demo.c".into(),
        "I64 Sq(I64 x) { return x * x; }\n".into(),
    );
    host.files.borrow_mut().insert(
        "notes/half.h".into(),
        "I64 Half(I64 x) { return x / 2; }\n".into(),
    );
    let mut shell = c_shell(&host);
    assert_eq!(shell.eval("#include \"demo.c\"\nSq(4)").text, "= 16");
    assert_eq!(
        shell
            .eval("#include \"/home/user/notes/half.h\"\nHalf(8)")
            .text,
        "= 4"
    );
    // Its definitions stay, as an entry's do.
    assert_eq!(shell.eval("Sq(5)").text, "= 25");
}

#[test]
fn an_include_outside_the_persons_folders_is_refused() {
    let host = FakeHost::new();
    host.files
        .borrow_mut()
        .insert("c/demo.c".into(), "I64 One() { return 1; }\n".into());
    let mut shell = c_shell(&host);
    for path in [
        "../../system/log",
        "/system/log",
        "/home/user/../x",
        "",
        "a//b",
    ] {
        let line = format!("#include \"{path}\"\nOne()");
        let refused = shell.eval(&line);
        assert!(!refused.ok, "{line}: {refused:?}");
        assert!(
            refused.text.contains("only a file in your own folders"),
            "{refused:?}"
        );
    }
    let missing = shell.eval("#include \"gone.c\"\n1");
    assert_eq!(
        missing.text,
        "#include \"gone.c\": /home/user/c/gone.c is not there"
    );
    // An include below the top is cc's to refuse, not read.
    let below = shell.eval("I64 a = 1;\n#include \"demo.c\"");
    assert!(!below.ok, "{below:?}");
}

/// A call in an included file was not asked for by the person's line: the
/// request is what they typed, so the call counts as added and asks, where
/// the same call typed runs (ADR 0002; #1192's review).
#[test]
fn a_call_in_an_included_file_asks_where_typed_it_runs() {
    let host = FakeHost::new();
    host.files
        .borrow_mut()
        .insert("notes/plan.txt".into(), "keep me".into());
    host.files.borrow_mut().insert(
        "c/tidy.c".into(),
        "I64 st = fs_trash(\"notes/plan.txt\");\n".into(),
    );
    let mut shell = c_shell(&host);
    let included = shell.eval("#include \"tidy.c\"");
    assert!(included.confirm.is_some(), "{included:?}");
    assert!(
        file(&host, "notes/plan.txt").is_some(),
        "not trashed without a yes"
    );
    let last = shell
        .log
        .get(shell.log.last_id())
        .map(|e| e.request.clone());
    assert!(
        last.as_ref()
            .is_none_or(|request| !request.contains("fs_trash")),
        "the log keeps the typed line: {last:?}"
    );
}

/// Each session saves, `:reset`s and loads back to the same answer, entry
/// by entry as typed; an include works where the file defines each name
/// once, and is refused by name where it does not (#1192's review).
#[test]
fn sessions_save_load_and_include_or_refuse_by_name() {
    struct Case {
        typed: &'static [&'static str],
        call: &'static str,
        answer: &'static str,
        twice: Option<&'static str>,
    }
    let cases = [
        Case {
            typed: &[
                "typedef struct { I64 x, y; } Point;",
                "Point Add(Point a, Point b) { Point r; r.x = a.x + b.x; r.y = a.y + b.y; return r; }",
                "I64 SumX(I64 a, I64 b) { Point p = {a, 0}; Point q = {b, 0}; return Add(p, q).x; }",
            ],
            call: "SumX(2, 5)",
            answer: "= 7",
            twice: None,
        },
        Case {
            typed: &[
                "I64 lo = 1, hi = 2;",
                "I64 hi = 9;",
                "I64 Both() { return lo + hi; }",
            ],
            call: "Both()",
            answer: "= 10",
            twice: Some("hi"),
        },
        Case {
            typed: &["#define N 4", "I64 cells[N];", "I64 Cells() { return N; }"],
            call: "Cells()",
            answer: "= 4",
            twice: None,
        },
        Case {
            typed: &["#include <kai.h>\nI64 Head() { return 3; }"],
            call: "Head()",
            answer: "= 3",
            twice: None,
        },
        Case {
            typed: &[
                "I64 Sq(I64 x) { return x * x; }",
                "I64 Twice(I64 x) { return Sq(x) * 2; }",
                "I64 Sq(I64 x) { return x + 1; }",
            ],
            call: "Twice(3)",
            answer: "= 8",
            twice: Some("Sq"),
        },
    ];
    for case in cases {
        let host = FakeHost::new();
        let mut shell = c_shell(&host);
        for entry in case.typed {
            let typed = shell.eval(entry);
            assert!(typed.ok, "{entry}: {typed:?}");
        }
        assert_eq!(shell.eval(case.call).text, case.answer, "{}", case.call);
        let saved = shell.eval(":save demo");
        assert!(saved.ok, "{}: {saved:?}", case.call);
        assert_eq!(shell.eval(":reset").text, "the C session is empty again");
        let loaded = shell.eval(":load demo");
        assert!(loaded.ok, "{}: {loaded:?}", case.call);
        assert_eq!(
            shell.eval(case.call).text,
            case.answer,
            "{} loaded",
            case.call
        );
        assert_eq!(shell.eval(":reset").text, "the C session is empty again");
        let included = shell.eval(&format!("#include \"demo.c\"\n{}", case.call));
        match case.twice {
            None => assert_eq!(included.text, case.answer, "{} included", case.call),
            Some(name) => assert_eq!(
                included.text,
                format!("#include \"demo.c\": it defines {name} twice; use :load")
            ),
        }
    }
}

/// A file that fails part way is taken back whole: nothing of it stays
/// defined.
#[test]
fn a_load_that_fails_part_way_is_taken_back_whole() {
    let host = FakeHost::new();
    host.files.borrow_mut().insert(
        "c/bad.c".into(),
        "#include <kai.h>\n//- entry\nI64 Good() { return 1; }\n//- entry\nI64 Bad() { return nope; }\n".into(),
    );
    let mut shell = c_shell(&host);
    let loaded = shell.eval(":load bad");
    assert!(
        !loaded.ok && loaded.text.contains("(entry 2)"),
        "{loaded:?}"
    );
    assert!(!shell.eval("Good()").ok, "the first entry was taken back");
    assert_eq!(shell.eval("undo").text, "nothing to undo at c>");
}

/// A save the fs refuses or cuts short says so, logs nothing and leaves the
/// file as it was (#1192's review).
#[test]
fn a_failed_save_logs_nothing_and_keeps_the_file() {
    let host = FakeHost::new();
    host.files
        .borrow_mut()
        .insert("c/demo.c".into(), "I64 Old() { return 0; }\n".into());
    let mut shell = c_shell(&host);
    assert!(shell.eval("I64 One() { return 1; }").ok);
    for why in ["Exists: refused", "Io: the disk failed mid-write"] {
        let logged = shell.log.last_id();
        *host.fs_fail.borrow_mut() = Some(why.into());
        let saved = shell.eval(":save demo");
        assert!(!saved.ok && saved.text.contains(why), "{saved:?}");
        assert_eq!(shell.log.last_id(), logged, "nothing logged");
        assert_eq!(
            file(&host, "c/demo.c").unwrap(),
            "I64 Old() { return 0; }\n"
        );
    }
}

/// An entry that included a file is kept as typed: a later save writes its
/// `#include` line, not the file's text, and a load reads the file again
/// (#1192's re-check).
#[test]
fn an_included_entry_is_saved_as_typed() {
    let host = FakeHost::new();
    host.files
        .borrow_mut()
        .insert("c/lib.c".into(), "I64 Seven() { return 7; }\n".into());
    let mut shell = c_shell(&host);
    assert_eq!(shell.eval("#include \"lib.c\"\nI64 g = Seven();").text, "");
    let saved = shell.eval(":save both");
    assert!(saved.ok, "{saved:?}");
    let text = file(&host, "c/both.c").unwrap();
    assert!(text.contains("#include \"lib.c\""), "{text}");
    assert!(!text.contains("return 7"), "{text}");
    assert_eq!(shell.eval(":reset").text, "the C session is empty again");
    assert!(shell.eval(":load both").ok);
    assert_eq!(shell.eval("g").text, "= 7", "its initializer ran on load");
}

/// A load refused for an initializer that calls kai leaves the live session
/// exactly as it was: its globals, its definitions and its undo (#1192's
/// re-check).
#[test]
fn a_refused_load_leaves_the_live_session_as_it_was() {
    let host = FakeHost::new();
    host.files.borrow_mut().insert(
        "c/calls.c".into(),
        "//- entry\nI64 Good() { return 1; }\n//- entry\nI64 st = fs_create(\"x.txt\", \"y\");\n"
            .into(),
    );
    let mut shell = c_shell(&host);
    assert!(shell.eval("I64 total = 42;").ok);
    let loaded = shell.eval(":load calls");
    assert!(
        !loaded.ok && loaded.text.contains("an initializer calls kai"),
        "{loaded:?}"
    );
    assert!(file(&host, "x.txt").is_none(), "no call ran");
    assert_eq!(shell.eval("total").text, "= 42");
    assert!(!shell.eval("Good()").ok, "nothing of the file stays");
    let undone = shell.eval("undo");
    assert!(undone.text.starts_with("undid entry 1"), "{undone:?}");
}

/// A restart does not replay an entry whose included file changed since it
/// ran, and says which (#1192's re-check).
#[test]
fn a_restart_tells_of_an_include_changed_since() {
    let host = FakeHost::new();
    host.files
        .borrow_mut()
        .insert("c/lib.c".into(), "I64 Seven() { return 7; }\n".into());
    let mut shell = c_shell(&host);
    assert!(
        shell
            .eval("#include \"lib.c\"\nI64 Eight() { return Seven() + 1; }")
            .ok
    );
    assert!(shell.eval("I64 Nine() { return 9; }").ok);
    host.files
        .borrow_mut()
        .insert("c/lib.c".into(), "I64 Seven() { return 70; }\n".into());
    start_cc(&host);
    let after = shell.eval("Nine()");
    assert!(
        after
            .text
            .contains("entry 1's #include files changed since it ran; not restored"),
        "{after:?}"
    );
    assert!(after.text.ends_with("= 9"), "{after:?}");
    assert!(!shell.eval("Seven()").ok, "the changed file never ran");
}

/// An entry that used what a skipped entry defined is named too, not
/// dropped without a word (#1192's review).
#[test]
fn a_restart_names_an_entry_that_used_a_skipped_one() {
    let host = FakeHost::new();
    host.files
        .borrow_mut()
        .insert("c/lib.c".into(), "I64 Seven() { return 7; }\n".into());
    let mut shell = c_shell(&host);
    assert!(
        shell
            .eval("#include \"lib.c\"\nI64 Eight() { return Seven() + 1; }")
            .ok
    );
    assert!(shell.eval("I64 Ten() { return Eight() + 2; }").ok);
    host.files
        .borrow_mut()
        .insert("c/lib.c".into(), "I64 Seven() { return 70; }\n".into());
    start_cc(&host);
    let after = shell.eval("1");
    assert!(
        after.text.contains("entry 1's #include files changed"),
        "{after:?}"
    );
    assert!(after.text.contains("entry 2 did not replay: "), "{after:?}");
    assert!(after.text.contains("; not restored"), "{after:?}");
}

/// A :load right after cc restarted opens the session again, as an entry
/// does, and loads (#1192's review).
#[test]
fn a_load_after_a_restart_opens_the_session_again() {
    let host = FakeHost::new();
    host.files
        .borrow_mut()
        .insert("c/lib.c".into(), "I64 One() { return 1; }\n".into());
    let mut shell = c_shell(&host);
    assert!(shell.eval("I64 Two() { return 2; }").ok);
    start_cc(&host);
    let loaded = shell.eval(":load lib");
    assert!(loaded.ok, "{loaded:?}");
    assert!(
        loaded
            .text
            .starts_with("cc restarted; restored 1 definitions"),
        "{loaded:?}"
    );
    assert_eq!(shell.eval("One() + Two()").text, "= 3");
}

/// :defs after cc restarted shows the restart's note first.
#[test]
fn defs_after_a_restart_shows_the_note() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    assert!(shell.eval("I64 Two() { return 2; }").ok);
    *host.cc.borrow_mut() = None;
    assert!(!shell.eval("Two()").ok);
    start_cc(&host);
    let defs = shell.eval(":defs");
    assert!(
        defs.text
            .starts_with("cc restarted; restored 1 definitions"),
        "{defs:?}"
    );
    assert!(defs.text.contains("Two"), "{defs:?}");
}

/// cc's reply as the wire writes it: one error record.
fn cc_error(text: &[u8]) -> Vec<u8> {
    let mut reply = promptcore::wire::Reply::new(kabi::MAX_MESSAGE);
    reply
        .push(&promptcore::wire::Record::Err { line: 0, text })
        .unwrap();
    reply.into_bytes()
}

/// A load whose reply does not parse fails, and the session stays as it
/// was: cc did not restart, so nothing resets (#1216's review).
#[test]
fn an_unparsable_load_reply_keeps_the_session() {
    let host = FakeHost::new();
    host.files
        .borrow_mut()
        .insert("c/lib.c".into(), "I64 One() { return 1; }\n".into());
    let mut shell = c_shell(&host);
    assert!(shell.eval("I64 total = 42;").ok);
    host.cc_script.borrow_mut().push_back(b"garbage".to_vec());
    let loaded = shell.eval(":load lib");
    assert!(
        !loaded.ok && loaded.text.contains("did not parse"),
        "{loaded:?}"
    );
    let after = shell.eval("total");
    assert_eq!(after.text, "= 42", "no restart, no reset: {after:?}");
    assert!(shell.eval("undo").text.starts_with("undid entry 1"));
}

/// cc restarted before a load whose second entry fails: the take-back
/// removes only what the load added, the person's entries stay, and the
/// restart's note is in the reply (#1216's review).
#[test]
fn a_restart_mid_load_takes_back_only_the_loads_entries() {
    let host = FakeHost::new();
    host.files.borrow_mut().insert(
        "c/half.c".into(),
        "//- entry\nI64 Good() { return 1; }\n//- entry\nI64 Bad() { return nope; }\n".into(),
    );
    let mut shell = c_shell(&host);
    assert!(shell.eval("I64 Keep() { return 5; }").ok);
    // The first load request finds no session, as after a restart.
    host.cc_script
        .borrow_mut()
        .push_back(cc_error(b"no session is open"));
    let loaded = shell.eval(":load half");
    assert!(!loaded.ok, "{loaded:?}");
    assert!(
        loaded
            .text
            .starts_with("cc restarted; restored 1 definitions"),
        "{loaded:?}"
    );
    assert!(loaded.text.contains("(entry 2)"), "{loaded:?}");
    assert_eq!(shell.eval("Keep()").text, "= 5", "the person's entry stays");
    assert!(!shell.eval("Good()").ok, "the load's entry was taken back");
}
