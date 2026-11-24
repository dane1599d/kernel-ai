//! A line whose file action `fs` would not commit (#1306's review): its
//! entries lose those file changes, so an undo of the line never restores
//! the later action that gets the same number, and the person is told.

use super::*;
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
