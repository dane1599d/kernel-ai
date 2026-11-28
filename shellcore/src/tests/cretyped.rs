//! A `retyped` record from the real `cc`, end to end (`Record::Retyped` in
//! cmode/mod.rs's `c_drive`): the reply says the function changed type, and
//! only for a function, never a macro or a tag in the same entry; a caller
//! defined before keeps the old body; and the entry counts as one that
//! defined something, so `undo` reverts it.

use super::cmode::start_cc;
use super::*;

const CHANGED: &str = "f changed type; code that already called it keeps the old one";

fn c_shell(host: &Rc<FakeHost>) -> Shell {
    start_cc(host);
    let mut shell = shell(host);
    assert!(shell.eval(":c").ok);
    for entry in ["I64 f() { return 1; }", "I64 g() { return f(); }"] {
        assert!(shell.eval(entry).ok, "{entry}");
    }
    shell
}

/// An entry that defines a macro and a tag and retypes `f`: the line is
/// said for `f` alone, and `g`, defined before, keeps calling the old `f`.
#[test]
fn a_retyped_function_beside_a_macro_and_a_tag_is_said_for_the_function_alone() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    let retyped = shell.eval("#define K 7\nstruct P { I64 x; };\nF64 f() { return 2.5; }");
    assert!(retyped.ok, "{retyped:?}");
    assert_eq!(retyped.text, CHANGED);
    assert_eq!(shell.eval("g()").text, "= 1");
    assert_eq!(shell.eval("f()").text, "= 2.5");
    assert_eq!(shell.eval("K").text, "= 7");
}

/// A macro or a tag defined again is no change of type: cc sends `retyped`
/// for functions only, and the shell says nothing.
#[test]
fn a_macro_or_a_tag_defined_again_says_no_change_of_type() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    for entry in [
        "#define K 7",
        "struct P { I64 x; };",
        "#define K 8",
        "struct P { I64 x; I64 y; };",
    ] {
        let defined = shell.eval(entry);
        assert!(defined.ok, "{entry}: {defined:?}");
        assert!(
            !defined.text.contains("changed type"),
            "{entry}: {defined:?}"
        );
    }
    assert_eq!(shell.eval("K").text, "= 8");
    assert_eq!(shell.eval("sizeof(struct P)").text, "= 16");
}

/// The retyping entry defined something, so `undo` takes it back, past the
/// calls after it, and `f` is the old one again.
#[test]
fn undo_reverts_the_entry_that_retyped_a_function() {
    let host = FakeHost::new();
    let mut shell = c_shell(&host);
    assert_eq!(shell.eval("F64 f() { return 2.5; }").text, CHANGED);
    assert_eq!(shell.eval("f()").text, "= 2.5");
    let undone = shell.eval("undo");
    assert!(
        undone.text.contains("its definitions are gone"),
        "{undone:?}"
    );
    assert_eq!(shell.eval("f()").text, "= 1");
    assert_eq!(shell.eval("g()").text, "= 1");
}
