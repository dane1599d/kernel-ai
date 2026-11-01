//! A file of the person's own, edited with the service's commands and the screen.

use crate::file::{COMPONENT, FileEditor, MAX_BYTES, NOTHING_TO_BUILD};
use crate::ipc::{At, Ipc, encode_ipc};
use crate::screen::{Input, Key, Screen};
use alloc::format;
use alloc::string::String;

const PATH: &str = "/home/user/todo.txt";
const NAME: &str = "home/user/todo.txt";

fn open(text: &str) -> FileEditor {
    FileEditor::open(PATH, text.as_bytes()).unwrap()
}

fn cmd(editor: &mut FileEditor, line: &str) -> String {
    editor.handle(&encode_ipc(&Ipc::Cmd(line)))
}

#[test]
fn a_file_views_as_the_service_shows_a_component_s_source() {
    let mut editor = open("buy milk\ncall Ann\nsleep\n");
    assert_eq!(
        cmd(&mut editor, "view 1,2"),
        format!("ok view file disk {NAME} 3 1 -\n1:buy milk\n2:call Ann")
    );
    assert_eq!(editor.path(), PATH);
    assert!(!editor.changed());
}

#[test]
fn an_edit_changes_the_bytes_and_keeps_every_line_ending() {
    let mut editor = open("a\r\nb\nc");
    assert_eq!(cmd(&mut editor, "c 2 B"), format!("ok {NAME}:2: B"));
    assert_eq!(editor.bytes(), b"a\r\nB\nc");
    assert!(editor.changed());
    assert_eq!(cmd(&mut editor, "status"), "ok file disk: edited");
    assert!(cmd(&mut editor, "diff").starts_with("ok changes since file disk"));
    // The OS wrote them: they are the disk's, and nothing differs from it.
    editor.saved();
    assert!(!editor.changed());
    assert_eq!(cmd(&mut editor, "status"), "ok file disk: unchanged");
    assert_eq!(cmd(&mut editor, "diff"), "ok no changes since file disk");
    // A later edit is against what was saved, and `reset` comes back to it.
    cmd(&mut editor, "c 1 A");
    assert!(editor.changed());
    cmd(&mut editor, "reset");
    assert_eq!(editor.bytes(), b"a\r\nB\nc");
}

#[test]
fn there_is_nothing_to_build_in_a_file() {
    let mut editor = open("x\n");
    for line in ["build", "try"] {
        assert_eq!(cmd(&mut editor, line), format!("err {NOTHING_TO_BUILD}"));
    }
}

#[test]
fn an_empty_file_has_one_empty_line_to_type_into() {
    let mut editor = FileEditor::open("/home/user/new.txt", b"").unwrap();
    assert_eq!(
        cmd(&mut editor, "view 1,5"),
        "ok view file disk home/user/new.txt 1 1 -\n1:"
    );
    assert!(!editor.changed());
    assert_eq!(editor.bytes(), b"");
    cmd(&mut editor, "c 1 hi");
    assert_eq!(editor.bytes(), b"hi");
}

#[test]
fn a_file_that_is_not_text_or_too_big_or_oddly_named_is_refused() {
    let refused = |path: &str, bytes: &[u8]| FileEditor::open(path, bytes).err().unwrap();
    assert!(refused(PATH, &[0xff, 0xfe, 0x00]).contains("is not text"));
    assert!(refused("/home/user/a b.txt", b"x").contains("spaces"));
    assert!(refused("/home/user/a:b", b"x").contains("drive letters"));
    assert!(refused("/home/../etc", b"x").contains(".. segments"));
    let big = alloc::vec![b'x'; MAX_BYTES + 1];
    assert!(refused(PATH, &big).contains("KiB"));
    // A file right at the limit opens (one long line is a line).
    let most = alloc::vec![b'x'; MAX_BYTES];
    assert!(FileEditor::open(PATH, &most).is_ok());
}

#[test]
fn an_open_request_moves_the_cursor_inside_the_one_file() {
    let mut editor = open("a\nb\nc\n");
    let at = |path, line| Ipc::Open {
        component: COMPONENT,
        label: None,
        at: Some(At { path, line }),
    };
    assert_eq!(
        editor.handle(&encode_ipc(&at(NAME, 3))),
        format!("ok editing file disk at {NAME}:3")
    );
    assert_eq!(
        editor.handle(&encode_ipc(&at("other/file", 1))),
        "err other/file is not the file open"
    );
    let plain = Ipc::Open {
        component: COMPONENT,
        label: None,
        at: None,
    };
    assert_eq!(
        editor.handle(&encode_ipc(&plain)),
        format!("ok editing file disk: {NAME}, 3 lines")
    );
}

#[test]
fn a_sync_or_a_replay_is_not_for_a_file() {
    let mut editor = open("x\n");
    let replay = Ipc::Replay {
        from: 0,
        entries: "",
    };
    assert!(editor.handle(&encode_ipc(&replay)).starts_with("err "));
    assert!(editor.handle(b"garbage").starts_with("err "));
}

/// A screen over a file, answered the way the app answers it: at once.
struct Rig {
    screen: Screen,
    editor: FileEditor,
}

impl Rig {
    fn new(text: &str) -> Self {
        let mut rig = Self {
            screen: Screen::new(10, 60),
            editor: open(text),
        };
        rig.screen.set_file_mode(true);
        rig.screen.open(COMPONENT, None, Some((NAME, 1)));
        rig.pump();
        rig
    }

    fn pump(&mut self) {
        for _ in 0..200 {
            let Some(out) = self.screen.next_request() else {
                return;
            };
            let reply = self.editor.handle(&out.request.encode());
            self.screen.reply(out.id, &reply);
        }
        panic!("the screen never stopped asking");
    }

    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.screen.input(Input::Key(Key::Char(c)));
            self.pump();
        }
    }
}

#[test]
fn typed_text_reaches_the_file_when_ctrl_s_asks_for_a_save() {
    let mut rig = Rig::new("buy milk\ncall Ann\n");
    rig.type_text("* ");
    assert!(!rig.screen.take_save());
    // Ctrl-S sends the line being typed first, so the bytes have it.
    rig.screen.input(Input::Key(Key::Ctrl('s')));
    rig.pump();
    assert!(rig.screen.take_save());
    assert!(!rig.screen.take_save(), "a save is asked for once");
    assert_eq!(rig.editor.bytes(), b"* buy milk\ncall Ann\n");
    assert!(rig.editor.changed());
}

#[test]
fn ctrl_b_in_a_file_says_there_is_nothing_to_build() {
    let mut rig = Rig::new("x\n");
    rig.screen.input(Input::Key(Key::Ctrl('b')));
    rig.pump();
    let view = rig.screen.view();
    assert_eq!(view.status, NOTHING_TO_BUILD);
    assert!(view.status_error);
}

#[test]
fn ctrl_o_in_a_file_asks_for_a_path_and_enter_hands_it_over() {
    let mut rig = Rig::new("x\n");
    rig.screen.input(Input::Key(Key::Ctrl('o')));
    assert_eq!(rig.screen.view().prompt, Some(("open file", String::new())));
    rig.type_text("/home/user/b.txt");
    assert_eq!(rig.screen.take_open(), None, "not until Enter");
    rig.screen.input(Input::Key(Key::Enter));
    assert_eq!(
        rig.screen.take_open(),
        Some(String::from("/home/user/b.txt"))
    );
    assert_eq!(rig.screen.take_open(), None);
    // Esc leaves the prompt without a path.
    rig.screen.input(Input::Key(Key::Ctrl('o')));
    rig.screen.input(Input::Key(Key::Esc));
    assert_eq!(rig.screen.take_open(), None);
    assert!(rig.screen.view().prompt.is_none());
}

#[test]
fn ctrl_o_with_nothing_open_asks_for_a_path_too() {
    let mut screen = Screen::new(10, 60);
    screen.input(Input::Key(Key::Ctrl('o')));
    assert_eq!(screen.view().prompt, Some(("open file", String::new())));
}
