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
