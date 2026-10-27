//! P2's file rows (task 11), on the host: what a script reads is `Data`, each
//! change is one `fs` transaction, and `undo` restores them, compare-and-set.

use super::*;
use crate::script::{self, Ran};
use crate::undo::{self, Ids, Inverse, Refused};
use alloc::format;

/// Checks `source` against the file rows and runs it with `host`.
fn run(host: &Rc<FakeHost>, source: &str) -> Ran {
    let dyn_host: Rc<dyn Host> = host.clone();
    let answer = Answer {
        summary: String::new(),
        script: String::from(source),
    };
    let checked = script::check_against(&answer, &dyn_host, &DEFAULT_CONFIG, &agentabi::FS_ROWS)
        .unwrap_or_else(|refusal| panic!("{refusal}"));
    script::run(&checked, &dyn_host, &DEFAULT_CONFIG)
}

fn ids() -> Ids {
    Ids { made: 1, next: 2 }
}

#[test]
fn a_files_text_is_data_shown_only_through_show() {
    let host = FakeHost::new();
    host.files
        .borrow_mut()
        .insert("notes.txt".into(), "milk, eggs".into());
    let ran = run(
        &host,
        "let t = fs_read(\"notes.txt\");\nprint(t);\nshow(t);",
    );
    assert!(ran.ok, "{}", ran.text);
    // `print` gets the type's name; `show` is the one way the text comes out.
    assert!(
        ran.text.contains("Data") && ran.text.contains("milk, eggs"),
        "{}",
        ran.text
    );
    assert!(ran.done.is_empty());
}

#[test]
fn each_change_is_a_transaction_and_undo_restores_them_newest_first() {
    let host = FakeHost::new();
    let ran = run(
        &host,
        "fs_create(\"a.txt\", \"one\");\nfs_write(\"a.txt\", \"two\");\nfs_move(\"a.txt\", \"b.txt\");",
    );
    assert!(ran.ok, "{}", ran.text);
    let txns: Vec<_> = ran.done.iter().map(|d| d.inverse).collect();
    assert_eq!(txns, [1, 2, 3].map(|txn| Some(Inverse::Fs { txn })));
    assert_eq!(ran.done[0].what(), "file a.txt created (was absent)");
    for done in ran.done.iter().rev() {
        undo::revert(host.as_ref(), done.inverse.unwrap(), false, ids()).unwrap();
    }
    assert!(host.files.borrow().is_empty());
}

#[test]
fn create_never_overwrites() {
    let host = FakeHost::new();
    host.files
        .borrow_mut()
        .insert("a.txt".into(), "kept".into());
    let ran = run(&host, "fs_create(\"a.txt\", \"new\");");
    assert!(!ran.ok && ran.text.contains("fs_write"), "{}", ran.text);
    assert_eq!(host.files.borrow()["a.txt"], "kept");
}

#[test]
fn undo_over_a_later_change_asks_and_its_redo_restores_the_restore() {
    let host = FakeHost::new();
    let ran = run(&host, "fs_write(\"a.txt\", \"mine\");");
    let inverse = ran.done[0].inverse.unwrap();
    // A later change to the same files: restoring would undo it too.
    host.files
        .borrow_mut()
        .insert("a.txt".into(), "later".into());
    assert_eq!(
        undo::revert(host.as_ref(), inverse, false, ids()).map(|_| ()),
        Err(Refused::Moved(String::from("a later change touched them")))
    );
    // Put back anyway, then redo: the restore was a transaction of its own.
    let (text, redo) = undo::revert(host.as_ref(), inverse, true, ids()).unwrap();
    assert_eq!(text, "files back as they were");
    assert!(host.files.borrow().is_empty());
    undo::revert(host.as_ref(), redo, false, ids()).unwrap();
    assert_eq!(host.files.borrow()["a.txt"], "later");
}

#[test]
fn trash_restore_and_emptying_the_trash() {
    let host = FakeHost::new();
    host.files.borrow_mut().insert("a.txt".into(), "x".into());
    let ran = run(&host, "fs_trash(\"a.txt\");\nfs_restore(\"a.txt\");");
    assert!(ran.ok, "{}", ran.text);
    assert_eq!(host.files.borrow()["a.txt"], "x");
    let ran = run(&host, "fs_trash(\"a.txt\");\nfs_empty_trash();");
    assert!(ran.ok, "{}", ran.text);
    // Emptying has no undo, so its change carries no inverse.
    assert_eq!(ran.done.last().map(|d| d.inverse), Some(None));
    assert!(host.files.borrow().is_empty(), "{:?}", host.files.borrow());
    let _ = format!("{:?}", ran.done);
}
