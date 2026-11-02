//! The editor's file door in the shell (P4): a save is one logged action with
//! undo and the log never holds the text; only the editor may use it; a write
//! needs a person; and no script can reach it.

use super::*;
use crate::door::{DOOR_APP, MAX_BYTES};
use llmproto::script::Answer;

const PATH: &str = "/home/user/todo.txt";

fn setup() -> (Rc<FakeHost>, Shell) {
    let host = FakeHost::new();
    host.files
        .borrow_mut()
        .insert("todo.txt".into(), "old text\n".into());
    let shell = shell(&host);
    (host, shell)
}

#[test]
fn a_save_is_one_logged_action_that_undo_puts_back_exactly() {
    let (host, mut shell) = setup();
    shell
        .door_write(DOOR_APP, PATH, b"new text\nsecond line\n", false, true)
        .unwrap();
    assert_eq!(host.files.borrow()["todo.txt"], "new text\nsecond line\n");
    // One entry in the log, written to disk with the change.
    let log = host.log_disk.borrow().clone().unwrap();
    assert!(
        log.contains("editor: save /home/user/todo.txt (21 bytes, sha256 "),
        "{log}"
    );
    assert!(log.contains("fs_write(\"todo.txt\", <21 bytes>)"), "{log}");
    // Never the text itself.
    assert!(!log.contains("second line"), "{log}");
    assert_eq!(log.matches("editor: save").count(), 1);
    // `undo` restores the file as it was, byte for byte.
    let reply = shell.eval("undo");
    assert!(reply.ok, "{}", reply.text);
    assert_eq!(host.files.borrow()["todo.txt"], "old text\n");
}

#[test]
fn a_new_file_is_made_and_never_overwrites() {
    let (host, mut shell) = setup();
    shell
        .door_write(DOOR_APP, "/home/user/new.txt", b"hi", true, true)
        .unwrap();
    assert_eq!(host.files.borrow()["new.txt"], "hi");
    let again = shell.door_write(DOOR_APP, "/home/user/new.txt", b"over", true, true);
    assert!(
        again.unwrap_err().contains("exists already"),
        "a new file never overwrites"
    );
    assert_eq!(host.files.borrow()["new.txt"], "hi");
    // Undoing the creation takes the file away again.
    assert!(shell.eval("undo").ok);
    assert!(!host.files.borrow().contains_key("new.txt"));
}

#[test]
fn a_save_without_a_person_s_key_or_click_writes_nothing() {
    let (host, mut shell) = setup();
    let writes = host.log_writes.get();
    let refused = shell.door_write(DOOR_APP, PATH, b"sneaky", false, false);
    assert!(refused.unwrap_err().contains("key or click"));
    assert_eq!(host.files.borrow()["todo.txt"], "old text\n");
    assert_eq!(host.log_writes.get(), writes, "no log entry");
}

#[test]
fn only_the_editor_uses_the_door() {
    let (host, mut shell) = setup();
    for app in ["notes", "files", "terminal-window", "shell", ""] {
        let wrote = shell.door_write(app, PATH, b"x", false, true);
        assert!(wrote.unwrap_err().contains("does not use the file door"));
        let read = shell.door_read(app, PATH);
        assert!(read.unwrap_err().contains("does not use the file door"));
    }
    assert_eq!(host.files.borrow()["todo.txt"], "old text\n");
}

#[test]
fn a_path_outside_the_users_files_is_refused() {
    let (host, mut shell) = setup();
    for path in [
        "/system/log",
        "/home/user",
        "/home/user/../etc",
        "todo.txt",
        "/home/other/x",
    ] {
        assert!(
            shell.door_write(DOOR_APP, path, b"x", false, true).is_err(),
            "{path}"
        );
        assert!(shell.door_read(DOOR_APP, path).is_err(), "{path}");
    }
    assert_eq!(host.files.borrow().len(), 1);
}

#[test]
fn text_that_is_not_text_or_too_big_is_not_written() {
    let (host, mut shell) = setup();
    let bad = shell.door_write(DOOR_APP, PATH, &[0xff, 0xfe], false, true);
    assert!(bad.unwrap_err().contains("not be text"));
    let big = alloc::vec![b'x'; MAX_BYTES + 1];
    let huge = shell.door_write(DOOR_APP, PATH, &big, false, true);
    assert!(huge.unwrap_err().contains("at most"));
    assert_eq!(host.files.borrow()["todo.txt"], "old text\n");
}

#[test]
fn a_read_gives_the_whole_file_and_a_missing_one_says_so_for_the_editor() {
    let (host, mut shell) = setup();
    let writes = host.log_writes.get();
    assert_eq!(shell.door_read(DOOR_APP, PATH).unwrap(), b"old text\n");
    let missing = shell
        .door_read(DOOR_APP, "/home/user/none.txt")
        .unwrap_err();
    assert!(missing.starts_with("missing"), "{missing}");
    assert_eq!(
        host.log_writes.get(),
        writes,
        "a read is not a logged action"
    );
}

#[test]
fn a_user_who_withheld_the_row_from_the_editor_is_obeyed() {
    let (host, mut shell) = setup();
    *host.withheld_disk.borrow_mut() = Some("kai-withheld 1\neditor fs_write\n".into());
    let refused = shell.door_write(DOOR_APP, PATH, b"x", false, true);
    assert!(refused.unwrap_err().contains("does not declare"));
    assert_eq!(host.files.borrow()["todo.txt"], "old text\n");
    // The other rows are still allowed.
    assert!(shell.door_read(DOOR_APP, PATH).is_ok());
    // A file out of shape refuses everything (fail closed).
    *host.withheld_disk.borrow_mut() = Some("garbage".into());
    assert!(shell.door_read(DOOR_APP, PATH).is_err());
}

#[test]
fn no_script_can_reach_the_door() {
    // The model's script, the panel's and `shell_line` run through the checker,
    // and the door is no function in any of them: a call to one is unknown.
    let (host, _) = setup();
    let dyn_host: Rc<dyn Host> = host.clone();
    for source in [
        "fs_write(\"todo.txt\", door_text());",
        "door_write(\"todo.txt\", \"x\");",
        "door_text();",
        "editor_save(\"todo.txt\");",
    ] {
        let answer = Answer {
            summary: String::new(),
            script: String::from(source),
        };
        let checked = script::check(&answer, &dyn_host, &DEFAULT_CONFIG);
        assert!(checked.is_err(), "{source} was accepted");
        let checked =
            script::check_against(&answer, &dyn_host, &DEFAULT_CONFIG, &agentabi::FS_ROWS);
        assert!(
            checked.is_err(),
            "{source} was accepted among the file rows"
        );
    }
    assert_eq!(host.files.borrow()["todo.txt"], "old text\n");
}
