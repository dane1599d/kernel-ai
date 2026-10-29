//! wcc objects linked into a session and run on wasmi.

mod common;

use common::Runner;
use kailink::{Error, Symbol};

#[test]
fn an_expression_runs() {
    let mut r = Runner::new();
    assert_eq!(r.eval("expr", Some("__kai_run_1")), Some(7));
}

#[test]
fn a_later_entry_calls_an_earlier_definition() {
    let mut r = Runner::new();
    assert_eq!(r.link("use_sq").unwrap_err(), Error::Undefined("sq".into()));
    r.eval("define_sq", None);
    assert!(matches!(
        r.session.symbol("sq"),
        Some(Symbol::Func { slot: 1, .. })
    ));
    assert_eq!(r.eval("use_sq", Some("__kai_run_4")), Some(49));
}

#[test]
fn globals_keep_their_address_across_entries() {
    let mut r = Runner::new();
    r.eval("global", None);
    let Some(Symbol::Data { addr, size: 8 }) = r.session.symbol("counter").cloned() else {
        panic!("counter is an 8-byte data symbol");
    };
    assert!(addr >= r.session.layout().data_base);
    assert_eq!(r.eval("extern", Some("__kai_run_6")), Some(8));
    assert_eq!(r.eval("extern", Some("__kai_run_6")), Some(11));
}

#[test]
fn kai_imports_are_allowed_by_name_and_type() {
    let mut r = Runner::new();
    assert_eq!(r.eval("print", Some("__kai_run_7")), Some(5));
    assert_eq!(r.output(), "hello\n42\n");
    let refused = r.session.link(&common::fixture("print"), &|_, _| false);
    assert!(matches!(refused, Err(Error::Import { ref module, .. }) if module == "kai"));
    assert_eq!(r.eval("need", Some("__kai_run_13")), Some(43));
}

#[test]
fn function_pointers_and_statics() {
    let mut r = Runner::new();
    assert_eq!(r.eval("fnptr", Some("__kai_run_12")), Some(21));
    assert_eq!(r.eval("struct", Some("__kai_run_8")), Some(30));
    assert_eq!(r.eval("data_ptr", Some("__kai_run_p")), Some(7));
}

#[test]
fn the_notes_logic_runs() {
    let mut r = Runner::new();
    assert_eq!(r.eval("notes", Some("__kai_run_19")), Some(1));
    assert_eq!(r.output(), "ok 2 notes, 7 words\n");
}

#[test]
fn uses_definitions_from_three_entries() {
    let mut r = Runner::new();
    for name in ["define_sq", "global", "fib"] {
        r.eval(name, None);
    }
    assert_eq!(
        r.eval("uses_many", Some("__kai_run_20")),
        Some(49 + 5 + 6765)
    );
}

#[test]
fn redefinition_switches_callers_and_stored_pointers() {
    let mut r = Runner::new();
    r.eval("redefine0", None);
    r.eval("redefine1", None);
    assert_eq!(r.eval("redefine2", Some("__kai_run_r1")), Some(1010));
    let linked = r.link("redefine3").unwrap();
    assert!(linked.defs.iter().any(|d| d.name == "twice" && d.replaces));
    assert!(linked.retyped.is_empty());
    assert_eq!(
        linked.table_size,
        r.session.table_size(),
        "same type keeps the slot"
    );
    r.instantiate(&linked, None);
    r.session.commit(&linked);
    assert_eq!(r.eval("redefine4", Some("__kai_run_r2")), Some(1515));
}

#[test]
fn another_type_gets_a_new_slot_and_old_callers_keep_the_old_body() {
    let mut r = Runner::new();
    r.eval("define_sq", None);
    r.eval("use_sq", None);
    let linked = r.link("sq_i64").unwrap();
    assert_eq!(linked.retyped, vec![String::from("sq")]);
    let Some(Symbol::Func { slot: old, .. }) = r.session.symbol("sq").cloned() else {
        panic!()
    };
    r.instantiate(&linked, None);
    r.session.commit(&linked);
    let Some(Symbol::Func { slot: new, .. }) = r.session.symbol("sq").cloned() else {
        panic!()
    };
    assert_ne!(old, new);
    // use_sq's callers were linked against the I32 sq; a new use of it now fails.
    assert_eq!(
        r.link("use_sq").unwrap_err(),
        Error::TypeMismatch("sq".into())
    );
}

#[test]
fn nothing_is_committed_until_commit() {
    let r = Runner::new();
    let before = (r.session.table_size(), r.session.data_end());
    let linked = r.link("global").unwrap();
    assert!(linked.data_end > before.1);
    assert_eq!((r.session.table_size(), r.session.data_end()), before);
    assert!(r.session.symbol("counter").is_none());
    assert_eq!(
        r.link("extern").unwrap_err(),
        Error::Undefined("counter".into())
    );
}

#[test]
fn the_sum_loop_runs() {
    let mut r = Runner::new();
    let want = (0..10_000_000i64).fold(0i64, |s, i| s.wrapping_add(i ^ (i >> 3)));
    assert_eq!(r.eval("sum", Some("__kai_run_sum")), Some(want));
}

#[test]
fn not_an_object() {
    let r = Runner::new();
    assert!(matches!(
        r.session.link(b"", &common::allow),
        Err(Error::Parse(_))
    ));
    let plain = b"\0asm\x01\0\0\0";
    assert!(matches!(
        r.session.link(plain, &common::allow),
        Err(Error::Parse(_))
    ));
}

#[test]
fn the_table_limit_holds() {
    let layout = kailink::Layout {
        table_limit: 2,
        ..kailink::Layout::default()
    };
    let mut session = kailink::Session::new(layout);
    let first = session
        .link(&common::fixture("define_sq"), &common::allow)
        .unwrap();
    session.commit(&first);
    let full = session.link(&common::fixture("fib"), &common::allow);
    assert_eq!(full.unwrap_err(), Error::Full("table"));
}

#[test]
fn revert_puts_back_what_a_commit_replaced() {
    let mut r = Runner::new();
    let first = r.link("define_sq").unwrap();
    let first_commit = r.session.commit(&first);
    let old = r.session.symbol("sq").cloned();
    let again = r.link("sq_i64").unwrap();
    let commit = r.session.commit(&again);
    assert_ne!(r.session.symbol("sq").cloned(), old);
    let size = r.session.table_size();
    r.session.revert(commit);
    assert_eq!(r.session.symbol("sq").cloned(), old);
    // Slots stay reserved: a stored pointer may still reach them.
    assert_eq!(r.session.table_size(), size);
    r.session.revert(first_commit);
    assert!(r.session.symbol("sq").is_none());
}

#[test]
fn reserve_keeps_slots_and_data_and_defines_nothing() {
    let mut r = Runner::new();
    let linked = r.link("global").unwrap();
    r.session.reserve(&linked);
    assert!(r.session.symbol("counter").is_none());
    assert_eq!(r.session.data_end(), linked.data_end);
    let next = r.link("global").unwrap();
    assert!(next.data_end > linked.data_end);
}
