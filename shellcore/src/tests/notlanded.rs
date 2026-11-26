//! A line whose file action `fs` would not commit (#1306's review): its
//! entries lose those file changes, so an undo of the line never restores
//! the later action that gets the same number, and the person is told.

use super::*;
use crate::PanelResult;
use alloc::vec::Vec;

fn write(path: &str, text: &str) -> Result<llmproto::act::Call, llmproto::act::ActError> {
    let code = format!("fs_write(\\\"{path}\\\", \\\"{text}\\\");");
    let json = format!(r#"{{"name":"script","arguments":{{"code":"{code}"}}}}"#);
    Ok(llmproto::act::parse(&json).unwrap())
}

#[test]
fn an_undo_of_a_line_that_did_not_land_leaves_the_next_lines_files() {
    let (host, mut shell) =
        super::act::with_calls(alloc::vec![write("a.txt", "one"), write("b.txt", "two")]);
    host.one_action.set(true);
    host.commit_refused.set(true);
    let lost = shell.eval_console("write a.txt");
    assert!(lost.text.contains("did not land"), "{}", lost.text);
    assert!(host.files.borrow().is_empty(), "{:?}", host.files.borrow());
    // The next line's action gets the lost one's number, and commits.
    let kept = shell.eval_console("write b.txt");
    assert!(kept.ok, "{}", kept.text);
    assert_eq!(host.txns.borrow().len(), 1);
    let undo = shell.eval("undo #1");
    assert_eq!(
        host.files.borrow().get("b.txt").map(String::as_str),
        Some("two"),
        "{}",
        undo.text
    );
    // The log on disk says the lost line's files did not land.
    let disk = host.log_disk.borrow().clone().unwrap_or_default();
    assert!(disk.contains("did not land"), "{disk}");
}

#[test]
fn a_line_that_did_not_land_never_says_done() {
    let (host, mut shell) = super::act::with_calls(alloc::vec![write("a.txt", "one")]);
    host.one_action.set(true);
    host.commit_refused.set(true);
    let lost = shell.eval_console("write a.txt");
    assert!(!lost.ok, "{}", lost.text);
    assert!(lost.text.contains("did not land"), "{}", lost.text);
    assert!(!lost.text.contains("Done"), "{}", lost.text);
    // Said once, in the reply: the next line does not carry it.
    host.commit_refused.set(false);
    let next = shell.eval_console("1 + 1");
    assert!(!next.text.contains("did not land"), "{}", next.text);
}

#[test]
fn a_panel_line_that_did_not_land_never_says_done() {
    let (host, mut shell) = super::act::with_calls(alloc::vec![write("a.txt", "one")]);
    host.one_action.set(true);
    host.commit_refused.set(true);
    match shell.eval_panel("write a.txt") {
        PanelResult::Refused(why) => assert!(why.contains("did not land"), "{why}"),
        other => panic!("{other:?}"),
    }
}

/// shellcore's side of M1: the entry an undo leaves keeps its redo. The
/// FakeHost answers undo itself, without the shell's `Ledger`, so this
/// passes on 6995e213 too; the check that fails there is
/// `ledger::tests::a_refused_commit_restores_nothing_until_its_number_commits_again`.
#[test]
fn a_redo_after_a_line_that_did_not_land_restores() {
    let (host, mut shell) =
        super::act::with_calls(alloc::vec![write("c.txt", "zero"), write("a.txt", "one")]);
    host.one_action.set(true);
    assert!(shell.eval("write c.txt").ok);
    host.commit_refused.set(true);
    shell.eval("write a.txt");
    // Undo the first line: its restore commits as the lost line's number.
    let undone = shell.eval("undo #1");
    assert!(undone.ok, "{}", undone.text);
    assert!(host.files.borrow().is_empty(), "{:?}", host.files.borrow());
    // Its redo restores that restore.
    let redone = shell.eval("undo");
    assert!(redone.ok, "{}", redone.text);
    let files: Vec<_> = host.files.borrow().values().cloned().collect();
    assert_eq!(files, ["zero"]);
}

#[test]
fn a_line_that_did_not_land_keeps_its_output() {
    let code = r#"{"name":"script","arguments":{"code":"print(\"hello there\");\nfs_write(\"a.txt\", \"one\");"}}"#;
    let (host, mut shell) =
        super::act::with_calls(alloc::vec![Ok(llmproto::act::parse(code).unwrap())]);
    host.one_action.set(true);
    host.commit_refused.set(true);
    let lost = shell.eval_console("say hello and write a.txt");
    assert!(lost.text.contains("hello there"), "{}", lost.text);
    assert!(lost.text.contains("did not land"), "{}", lost.text);
    assert!(!lost.text.contains("Done"), "{}", lost.text);
}

/// The shell's own rollback is no loss (M1 on 0e4340d6): the failed run's
/// entry is written to the log on disk, and nothing says `fs` dropped it.
/// The FakeHost keeps the shell's own `Ledger`, as `Files` does, so this
/// fails if an abort counts as a loss again.
#[test]
fn a_rolled_back_line_is_logged_and_not_called_lost() {
    let throws = r#"{"name":"script","arguments":{"code":"fs_create(\"a.txt\", \"1\");\nfs_create(\"a.txt\", \"2\");"}}"#;
    let (host, mut shell) =
        super::act::with_calls(alloc::vec![Ok(llmproto::act::parse(throws).unwrap())]);
    host.one_action.set(true);
    let reply = shell.eval_console("create a.txt twice");
    assert!(reply.text.starts_with("Nothing changed"), "{}", reply.text);
    assert!(!reply.text.contains("did not land"), "{}", reply.text);
    let disk = host.log_disk.borrow().clone().unwrap_or_default();
    assert!(disk.contains("nothing changed"), "{disk}");
}

fn script(code: &str) -> Result<llmproto::act::Call, llmproto::act::ActError> {
    let json = format!(r#"{{"name":"script","arguments":{{"code":"{code}"}}}}"#);
    Ok(llmproto::act::parse(&json).unwrap())
}

/// M7: the panel's history keeps what a script printed, and leaves out the
/// note; the row says the policy change and that the file did not land.
#[test]
fn a_panel_line_that_did_not_land_keeps_what_it_printed() {
    let code = "print(\\\"hello there\\\");\\nset_policy(\\\"round-robin\\\");\\nfs_write(\\\"a.txt\\\", \\\"one\\\");";
    let (host, mut shell) = super::act::with_calls(alloc::vec![script(code)]);
    host.one_action.set(true);
    host.commit_refused.set(true);
    let row = shell.eval_panel("set the policy to round robin, print hello there and write a.txt");
    let said = shell.take_panel_said();
    assert_eq!(said.as_deref(), Some("hello there"), "{row:?}");
    match row {
        PanelResult::Refused(why) => {
            assert!(why.contains("did not land"), "{why}");
            assert!(why.contains("policy"), "the policy change stays: {why}");
            assert!(!why.contains(".;"), "{why}");
            assert!(!why.contains("file a.txt"), "{why}");
        }
        other => panic!("{other:?}"),
    }
}

/// A line that failed on its own and did not land: the panel's history gets
/// none of code's error lines (nor "file a.txt written", which is not on
/// disk), though the row says the files did not land.
#[test]
fn a_failed_panel_line_that_did_not_land_puts_no_error_lines_in_the_history() {
    use crate::undo::{Done, Inverse};
    let answer = Ok(Answer {
        summary: String::from("Does it."),
        script: String::from("print(cpus());"),
    });
    let (host, mut shell) = super::scripts::with_answers(alloc::vec![answer]);
    host.one_action.set(true);
    // The file write is in the line's open action, which the rollback cannot
    // put back: the change after it has no inverse, so the rollback stops.
    let txn = host
        .fs_change(FsChange::Write {
            path: "a.txt",
            text: "one",
        })
        .unwrap();
    let change = |action: &str, res: &str, inverse| Done {
        action: action.into(),
        owner: "kernel".into(),
        res: res.into(),
        before: String::new(),
        after: "written".into(),
        inverse,
    };
    let ran = crate::script::Ran {
        ok: false,
        text: String::from("printed\nboom: it failed"),
        done: alloc::vec![
            change("fs_write", "file a.txt", Some(Inverse::Fs { txn })),
            change("fs_empty_trash", "trash", None),
        ],
        value: None,
        shown: Vec::new(),
        failed: Some(String::from("boom: it failed")),
        withheld: false,
        unanswered: None,
    };
    let wire = crate::apart::encode(&ran);
    *host.apart.borrow_mut() = Some(alloc::boxed::Box::new(move |_: &str| Ok(wire.clone())));
    host.commit_refused.set(true);
    let row = shell.eval_panel("? what is going on");
    let said = shell.take_panel_said();
    assert!(
        matches!(&row, PanelResult::Refused(why) if why.contains("did not land")),
        "{row:?}"
    );
    assert_eq!(said, None, "{said:?}");
}

/// A script that prints the shell's exact done line: the real one, written
/// after the output, is the one that goes.
#[test]
fn a_printed_copy_of_the_done_line_leaves_the_real_one_removed() {
    let code = "print(\\\"Done: file a.txt written (was its earlier text). undo, he said\\\");\\nfs_write(\\\"a.txt\\\", \\\"one\\\");";
    let (host, mut shell) = super::act::with_calls(alloc::vec![script(code)]);
    host.files.borrow_mut().insert("a.txt".into(), "old".into());
    host.one_action.set(true);
    host.commit_refused.set(true);
    let lost = shell.eval_console("write a.txt");
    // The printed words stay whole; the shell's own line, after them, goes.
    assert!(
        lost.text
            .lines()
            .any(|l| l == "Done: file a.txt written (was its earlier text). undo, he said"),
        "{}",
        lost.text
    );
    assert_eq!(
        lost.text.matches("file a.txt written").count(),
        1,
        "the real Done stays: {}",
        lost.text
    );
    assert!(lost.text.contains("did not land"), "{}", lost.text);
}

/// A landed panel line whose file name holds a line break: its done line,
/// two lines long, is not Kai's words in the history.
#[test]
fn a_landed_panel_line_with_a_line_break_in_a_name_keeps_its_done_out_of_the_history() {
    let code = "print(\\\"hello there\\\");\\nfs_write(\\\"x\\\\ny.txt\\\", \\\"one\\\");";
    let (host, mut shell) = super::act::with_calls(alloc::vec![script(code)]);
    host.one_action.set(true);
    let _row = shell.eval_panel("write x y.txt");
    assert_eq!(
        shell.take_panel_said().as_deref(),
        Some("hello there"),
        "{:?}",
        host.files.borrow()
    );
}
