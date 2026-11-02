//! The editor on a file of the person's own: Ctrl-O's path, the file coming and
//! going whole through a fake file door, and what Ctrl-S and Ctrl-B do. The door
//! here is a map of paths to bytes; the OS side comes later.

use comp_editor_ui::{CODE, STATUS, TITLE};
use comp_editor_window::{EditorWindow, Host, keys};
use comp_ui_bind::Sink;
use comp_ui_bind::tree::Entry;
use compsdk::Component;
use editcore::file::{MAX_BYTES, NOTHING_TO_BUILD};
use std::collections::BTreeMap;
use uikit::widget::{NodeId, UiError, Widget};

const PATH: &str = "/home/user/todo.txt";

/// What the editor asked the door.
#[derive(Clone, Debug)]
enum Req {
    Read(String),
    Write {
        path: String,
        data: Vec<u8>,
        create: bool,
    },
}

#[derive(Default)]
struct Os {
    puts: Vec<Entry>,
    /// Requests out and not answered: (ticket, request).
    pending: Vec<(u32, Req)>,
    /// Every request ever made, as text.
    log: Vec<String>,
    next_ticket: u32,
    /// The disk behind the door.
    disk: BTreeMap<String, Vec<u8>>,
    /// Why the next write is refused, if it is.
    refuse: Option<&'static str>,
    /// The `create` flag and length of every write that was done.
    writes: Vec<(bool, usize)>,
}

impl Sink for Os {
    fn open(&mut self, _title: &str, _width: u16, _height: u16) -> Result<u32, UiError> {
        Ok(7)
    }

    fn put(&mut self, _window: u32, e: &Entry) -> Result<(), UiError> {
        self.puts.retain(|p| p.id != e.id);
        self.puts.push(e.clone());
        Ok(())
    }

    fn remove(&mut self, _window: u32, node: NodeId) -> Result<(), UiError> {
        self.puts.retain(|p| p.id != node);
        Ok(())
    }

    fn commit(&mut self, _window: u32) -> Result<(), UiError> {
        Ok(())
    }

    fn close(&mut self, _window: u32) -> Result<(), UiError> {
        Ok(())
    }
}

impl Os {
    fn ask(&mut self, text: String, req: Req) -> u32 {
        self.next_ticket += 1;
        self.log.push(text);
        self.pending.push((self.next_ticket, req));
        self.next_ticket
    }
}

impl Host for Os {
    fn send(&mut self, message: &[u8]) -> u32 {
        let text = format!("edit {}", String::from_utf8_lossy(message));
        self.next_ticket += 1;
        self.log.push(text);
        self.next_ticket
    }

    fn now_ms(&self) -> u64 {
        0
    }

    fn read_file(&mut self, path: &str) -> u32 {
        self.ask(format!("read {path}"), Req::Read(path.into()))
    }

    fn write_file(&mut self, path: &str, data: &[u8], create: bool) -> u32 {
        let how = if create { "new" } else { "old" };
        self.ask(
            format!("write {path} {} {how}", data.len()),
            Req::Write {
                path: path.into(),
                data: data.to_vec(),
                create,
            },
        )
    }
}

impl Os {
    fn label(&self, id: NodeId) -> String {
        let entry = self.puts.iter().find(|e| e.id == id).expect("a node");
        match &entry.widget {
            Widget::Label { text, .. } => text.clone(),
            other => panic!("{other:?}"),
        }
    }

    fn status(&self) -> String {
        self.label(STATUS)
    }

    fn title(&self) -> String {
        self.label(TITLE)
    }

    fn rows(&self) -> Vec<String> {
        let entry = self.puts.iter().find(|e| e.id == CODE).expect("the code");
        let Widget::Code(view) = &entry.widget else {
            panic!("not a code view");
        };
        view.rows.iter().map(|r| r.text.clone()).collect()
    }

    /// What the OS side of the door answers.
    fn door(&mut self, req: Req) -> Result<Vec<u8>, String> {
        match req {
            Req::Read(path) => self
                .disk
                .get(&path)
                .cloned()
                .ok_or_else(|| String::from("missing: no such file")),
            Req::Write { path, data, create } => {
                if let Some(why) = self.refuse {
                    return Err(why.into());
                }
                if create && self.disk.contains_key(&path) {
                    return Err("the file exists already".into());
                }
                self.writes.push((create, data.len()));
                self.disk.insert(path, data);
                Ok(Vec::new())
            }
        }
    }
}

/// Answers every request the editor has out, and those its answers lead to.
fn serve(app: &mut EditorWindow, os: &mut Os) {
    for _ in 0..64 {
        if os.pending.is_empty() {
            return;
        }
        let (ticket, req) = os.pending.remove(0);
        let outcome = os.door(req);
        app.on_reply(os, ticket, outcome).unwrap();
    }
    panic!("the editor keeps asking");
}

fn press(app: &mut EditorWindow, os: &mut Os, name: &str, text: &str, mods: u8) {
    app.on_key(os, name, text, mods).unwrap();
}

fn type_text(app: &mut EditorWindow, os: &mut Os, text: &str) {
    for c in text.chars() {
        let name = c.to_string();
        press(app, os, &name, &name, 0);
    }
}

/// Ctrl-O, the path, Enter; the door's answer in.
fn open_path(app: &mut EditorWindow, os: &mut Os, path: &str) {
    press(app, os, "o", "", keys::CTRL);
    type_text(app, os, path);
    press(app, os, "enter", "", 0);
    serve(app, os);
}

fn save(app: &mut EditorWindow, os: &mut Os) {
    press(app, os, "s", "", keys::CTRL);
    serve(app, os);
}

fn window() -> (EditorWindow, Os) {
    let mut app = EditorWindow::init();
    let mut os = Os::default();
    app.on_size(&mut os, 10, 60).unwrap();
    (app, os)
}

fn todo() -> Vec<u8> {
    b"buy milk\r\ncall Ann\nsleep".to_vec()
}

#[test]
fn an_editor_with_nothing_open_says_how_to_open_a_file() {
    let (_, os) = window();
    assert!(os.status().contains("Ctrl-O opens one"), "{}", os.status());
}

#[test]
fn ctrl_o_asks_for_a_path_and_the_file_comes_through_the_door() {
    let (mut app, mut os) = window();
    os.disk.insert(PATH.into(), todo());
    open_path(&mut app, &mut os, PATH);
    assert_eq!(os.log, [format!("read {PATH}")]);
    assert!(
        os.title().starts_with("file disk: home/user/todo.txt"),
        "{}",
        os.title()
    );
    assert_eq!(&os.rows()[..3], ["buy milk", "call Ann", "sleep"]);
    // The edit service was not asked about any of it.
    assert!(
        os.log.iter().all(|m| !m.starts_with("edit ")),
        "{:?}",
        os.log
    );
}

#[test]
fn a_path_that_is_not_there_opens_a_new_file_that_ctrl_s_makes() {
    let (mut app, mut os) = window();
    open_path(&mut app, &mut os, "/home/user/new.txt");
    assert!(os.title().contains("new.txt"), "{}", os.title());
    type_text(&mut app, &mut os, "hi");
    save(&mut app, &mut os);
    assert_eq!(os.disk["/home/user/new.txt"], b"hi");
    assert_eq!(os.writes, [(true, 2)], "made, not replaced");
    assert!(
        os.status()
            .starts_with("saved /home/user/new.txt (2 bytes)"),
        "{}",
        os.status()
    );
    // The next save replaces it.
    type_text(&mut app, &mut os, "!");
    save(&mut app, &mut os);
    assert_eq!(os.disk["/home/user/new.txt"], b"hi!");
    assert_eq!(os.writes.last(), Some(&(false, 3)));
}

#[test]
fn a_big_file_goes_in_and_out_whole() {
    let (mut app, mut os) = window();
    let line = "y".repeat(99) + "\n";
    let big = line.repeat(500).into_bytes();
    assert!(big.len() > 40_000 && big.len() < MAX_BYTES);
    os.disk.insert(PATH.into(), big.clone());
    open_path(&mut app, &mut os, PATH);
    os.log.clear();
    type_text(&mut app, &mut os, "A");
    save(&mut app, &mut os);
    assert_eq!(os.log, [format!("write {PATH} {} old", big.len() + 1)]);
    let saved = &os.disk[PATH];
    assert!(saved.starts_with(b"Ayyy"));
    assert_eq!(saved.len(), big.len() + 1);
    assert!(!os.title().ends_with('*'), "{}", os.title());
    assert!(os.status().starts_with("saved"), "{}", os.status());
    // Nothing changed since: another Ctrl-S asks nothing.
    os.log.clear();
    press(&mut app, &mut os, "s", "", keys::CTRL);
    assert!(os.log.is_empty());
    assert_eq!(os.status(), "nothing to save");
}

#[test]
fn a_file_over_what_the_editor_opens_is_refused_with_its_size() {
    let (mut app, mut os) = window();
    os.disk.insert(PATH.into(), vec![b'x'; MAX_BYTES + 1]);
    open_path(&mut app, &mut os, PATH);
    assert!(os.status().contains("KiB"), "{}", os.status());
    assert!(os.title().len() < 20, "no file opened: {}", os.title());
}

#[test]
fn a_refused_save_says_why_and_leaves_the_file_edited() {
    let (mut app, mut os) = window();
    os.disk.insert(PATH.into(), todo());
    open_path(&mut app, &mut os, PATH);
    type_text(&mut app, &mut os, "x");
    os.refuse = Some("fs_write asks first: ask the AI");
    save(&mut app, &mut os);
    assert_eq!(os.status(), "not saved: fs_write asks first: ask the AI");
    assert_eq!(os.disk[PATH], todo());
    // The edit is still there to save once the OS allows it.
    os.refuse = None;
    save(&mut app, &mut os);
    assert!(os.disk[PATH].starts_with(b"xbuy milk\r\n"));
}

#[test]
fn another_file_is_not_opened_over_edits_that_are_not_saved() {
    let (mut app, mut os) = window();
    os.disk.insert(PATH.into(), todo());
    os.disk.insert("/home/user/b.txt".into(), b"b\n".to_vec());
    open_path(&mut app, &mut os, PATH);
    type_text(&mut app, &mut os, "x");
    os.log.clear();
    open_path(&mut app, &mut os, "/home/user/b.txt");
    assert!(os.log.is_empty(), "{:?}", os.log);
    assert!(os.status().contains("not saved"), "{}", os.status());
    assert!(os.title().contains("todo.txt"));
    // Saved, it opens.
    save(&mut app, &mut os);
    open_path(&mut app, &mut os, "/home/user/b.txt");
    assert!(os.title().contains("b.txt"), "{}", os.title());
}

#[test]
fn a_path_the_door_would_refuse_never_leaves_the_editor() {
    let (mut app, mut os) = window();
    for path in [
        "todo.txt",
        "/home/../etc/passwd",
        "/home/user/a b",
        "/system/log",
    ] {
        open_path(&mut app, &mut os, path);
        assert!(os.log.is_empty(), "{path}: {:?}", os.log);
        assert!(os.status().len() > 3);
    }
}

#[test]
fn the_door_failing_or_a_file_that_is_not_text_says_so() {
    let (mut app, mut os) = window();
    os.disk.insert(PATH.into(), vec![0xff, 0xfe, 0x00]);
    open_path(&mut app, &mut os, PATH);
    assert!(os.status().contains("is not text"), "{}", os.status());
    // The OS gives a reason.
    press(&mut app, &mut os, "o", "", keys::CTRL);
    type_text(&mut app, &mut os, "/home/user/c.txt");
    press(&mut app, &mut os, "enter", "", 0);
    let (ticket, _) = os.pending.remove(0);
    app.on_reply(&mut os, ticket, Err("the OS has no file door".into()))
        .unwrap();
    assert_eq!(os.status(), "the OS has no file door");
}

#[test]
fn ctrl_b_in_a_file_builds_nothing() {
    let (mut app, mut os) = window();
    os.disk.insert(PATH.into(), todo());
    open_path(&mut app, &mut os, PATH);
    os.log.clear();
    press(&mut app, &mut os, "b", "", keys::CTRL);
    serve(&mut app, &mut os);
    assert_eq!(os.status(), NOTHING_TO_BUILD);
    assert!(os.log.is_empty());
}

#[test]
fn ctrl_s_with_nothing_open_says_what_it_is_for() {
    let (mut app, mut os) = window();
    press(&mut app, &mut os, "s", "", keys::CTRL);
    assert!(os.status().contains("saves a file"), "{}", os.status());
    assert!(os.log.is_empty());
}

#[test]
fn the_agent_s_open_leaves_a_saved_file_for_a_component_and_not_an_edited_one() {
    let (mut app, mut os) = window();
    os.disk.insert(PATH.into(), todo());
    open_path(&mut app, &mut os, PATH);
    type_text(&mut app, &mut os, "x");
    let refused = app.act_open(&mut os, "notes", "src/lib.rs", 1);
    assert!(refused.unwrap_err().contains("not saved"));
    save(&mut app, &mut os);
    os.log.clear();
    app.act_open(&mut os, "notes", "src/lib.rs", 1).unwrap();
    // The edit service is asked, the file door is not.
    assert!(os.log[0].starts_with("edit "), "{:?}", os.log);
    assert!(
        os.log
            .iter()
            .all(|m| !m.starts_with("read ") && !m.starts_with("write ")),
        "{:?}",
        os.log
    );
}
