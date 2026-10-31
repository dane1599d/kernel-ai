//! The Files window on the host: golden display lists of its views, drawn by
//! `uikit` with `FixedAdvance`, the round trip from window events to the calls
//! the OS is asked for, and that every state's tree is one `uikit` accepts. Set
//! `UIKIT_BLESS=1` to rewrite the goldens, and review the diff.

use comp_files::fake::Queue;
use comp_files::{Files, Preview};
use comp_files_ui::{
    self as ui, Change, Entry, ITEM, LIST, MOVE, NAME, OPEN, REFRESH, RESTORE, TRASH, UP,
};
use std::path::PathBuf;
use uikit::a11y::Action;
use uikit::layout::FixedAdvance as M;
use uikit::widget::{UiEvent, Value, Widget};
use uikit::{Frame, Window};

fn golden(name: &str, text: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.txt"));
    if std::env::var_os("UIKIT_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{} is missing; run with UIKIT_BLESS=1", path.display()))
        .replace("\r\n", "\n");
    assert_eq!(text, want, "{name} differs from {}", path.display());
}

struct App {
    files: Files,
    os: Queue,
}

impl App {
    /// Home, listed.
    fn new() -> Self {
        let mut app = Self {
            files: Files::new(),
            os: Queue::default(),
        };
        app.files.start(&mut app.os);
        app.answer("docs/\nnotes.txt\nphoto one.png\nold/\n");
        app
    }

    fn answer(&mut self, text: &str) {
        let (ticket, _) = self.os.last().unwrap();
        self.files
            .done(&mut self.os, ticket, Ok(String::from(text)));
    }

    fn fail(&mut self, why: &str) {
        let (ticket, _) = self.os.last().unwrap();
        self.files
            .done(&mut self.os, ticket, Err(String::from(why)));
    }

    fn last(&self) -> String {
        self.os.last().unwrap().1
    }

    fn tree(&self) -> Vec<Entry> {
        ui::build(&self.files)
    }

    fn event(&mut self, e: &UiEvent) -> bool {
        ui::on_ui(&mut self.files, &mut self.os, e)
    }
}

/// Sends `changes` to a window, as comphost will through `kai:ui/view`.
fn apply(w: &mut Window, changes: &[Change]) {
    for c in changes {
        match c {
            Change::Put(e) => w.put(e.id, e.parent, e.layout, e.widget.clone()).unwrap(),
            Change::Remove(id) => w.remove(*id).unwrap(),
        }
    }
}

fn window(tree: &[Entry]) -> Window {
    let (w, h) = ui::SIZE;
    let mut win = Window::new(ui::TITLE_TEXT, w, h);
    apply(&mut win, &ui::diff(&[], tree));
    win
}

fn frame(tree: &[Entry]) -> Frame {
    window(tree).commit(&M)
}

fn a11y(f: &Frame) -> String {
    f.a11y
        .iter()
        .map(|n| {
            let value = n
                .value
                .as_ref()
                .map_or(String::new(), |v| format!(" = {v:?}"));
            let runs = n
                .bound
                .as_ref()
                .map_or(String::new(), |b| format!(" runs {b}"));
            format!("[{}] {} {:?}{value}{runs}\n", n.id, n.role.name(), n.name)
        })
        .collect()
}

#[test]
fn the_home_view() {
    let app = App::new();
    let f = frame(&app.tree());
    golden("home", &f.list.to_string());
    golden("home-a11y", &a11y(&f));
}

#[test]
fn a_file_selected_with_its_text() {
    let mut app = App::new();
    app.files.select(&mut app.os, "notes.txt").unwrap();
    app.answer("Groceries\n\tmilk\n\teggs\n");
    let f = frame(&app.tree());
    golden("file", &f.list.to_string());
    golden("file-a11y", &a11y(&f));
}

#[test]
fn a_folder_inside_another_with_a_failure() {
    let mut app = App::new();
    app.files.select(&mut app.os, "docs").unwrap();
    app.answer("2024/\nplan.md\n");
    app.files.open_selected(&mut app.os);
    app.answer("2024/\nplan.md\n");
    app.files.select(&mut app.os, "plan.md").unwrap();
    app.fail("plan.md: not text");
    let f = frame(&app.tree());
    golden("folder", &f.list.to_string());
    golden("folder-a11y", &a11y(&f));
}

#[test]
fn the_buttons_are_bound_to_the_rows_they_run() {
    let tree = App::new().tree();
    let bound = |id| match &tree.iter().find(|e| e.id == id).unwrap().widget {
        Widget::Button { action, .. } => action.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(bound(UP).as_deref(), Some("files_open"));
    assert_eq!(bound(OPEN).as_deref(), Some("files_open"));
    assert_eq!(bound(MOVE).as_deref(), Some("fs_move"));
    assert_eq!(bound(TRASH).as_deref(), Some("fs_trash"));
    assert_eq!(bound(RESTORE).as_deref(), Some("fs_restore"));
    assert_eq!(bound(REFRESH), None);
    // The file rows are the OS's, so the buttons show those rows' words.
    for name in ["fs_move", "fs_trash", "fs_restore"] {
        assert!(agentabi::ROWS.iter().any(|r| r.name == name), "{name}");
    }
}

#[test]
fn clicks_in_the_window_become_the_calls_the_os_is_asked_for() {
    let mut app = App::new();
    let old = app.tree();
    let mut win = window(&old);
    win.commit(&M);

    // The user clicks the second name.
    let picked = win
        .act(LIST, Action::SetValue, Some(Value::Index(2)), &M)
        .unwrap();
    for e in &picked {
        assert!(app.event(e));
    }
    assert_eq!(app.files.selected(), Some("notes.txt"));
    assert_eq!(app.last(), r#"fs_read {path: "notes.txt"}"#);
    app.answer("hello");

    // The name field edits, then Enter moves.
    app.event(&UiEvent::Changed(
        NAME,
        Value::Text(String::from("hello.txt")),
    ));
    assert!(app.event(&UiEvent::Submitted(NAME)));
    assert_eq!(
        app.last(),
        r#"fs_move {from: "notes.txt", to: "hello.txt"}"#
    );
    app.answer("");
    app.answer("docs/\nhello.txt\nphoto one.png\nold/\n");
    assert_eq!(app.files.selected(), Some("hello.txt"));
    app.answer("hello");

    assert!(app.event(&UiEvent::Pressed(TRASH)));
    assert_eq!(app.last(), r#"fs_trash {path: "hello.txt"}"#);
    app.answer("");
    app.answer("docs/\nphoto one.png\nold/\n");
    assert!(app.event(&UiEvent::Pressed(RESTORE)));
    assert_eq!(app.last(), r#"fs_restore {path: "hello.txt"}"#);

    // A folder: select, then Open, then Up.
    app.event(&UiEvent::Pressed(REFRESH));
    app.answer("docs/\nphoto one.png\nold/\n");
    app.event(&UiEvent::Changed(LIST, Value::Index(0)));
    app.answer("a.md");
    app.event(&UiEvent::Pressed(OPEN));
    assert_eq!(app.last(), r#"fs_list {path: "docs"}"#);
    app.answer("a.md");
    assert_eq!(app.files.dir(), "docs");
    app.event(&UiEvent::Pressed(UP));
    assert_eq!(app.last(), r#"fs_list {path: ""}"#);

    // Events the window does not use change nothing.
    assert!(!app.event(&UiEvent::Pressed(9_999)));
    assert!(!app.event(&UiEvent::Changed(LIST, Value::Index(500))));
    assert!(!app.event(&UiEvent::CloseRequested));
}

#[test]
fn every_state_is_a_tree_the_window_accepts_and_the_diff_sends_less() {
    let mut app = App::new();
    let mut shown: Vec<Entry> = Vec::new();
    let mut win = window(&shown);
    let step = |app: &mut App, shown: &mut Vec<Entry>, win: &mut Window, i: usize| {
        let new = app.tree();
        let changes = ui::diff(shown, &new);
        if i > 0 {
            assert!(changes.len() < new.len(), "step {i} sent the whole tree");
        }
        apply(win, &changes);
        assert_eq!(win.commit(&M).list, frame(&new).list, "step {i}");
        *shown = new;
    };
    step(&mut app, &mut shown, &mut win, 0);
    app.files.select(&mut app.os, "notes.txt").unwrap();
    step(&mut app, &mut shown, &mut win, 1);
    app.answer("one\ntwo\nthree");
    step(&mut app, &mut shown, &mut win, 2);
    app.files.select(&mut app.os, "docs").unwrap();
    app.answer("a/\nb.txt");
    step(&mut app, &mut shown, &mut win, 3);
    app.files.refresh(&mut app.os);
    app.fail("the disk went away");
    step(&mut app, &mut shown, &mut win, 4);
    app.files.select(&mut app.os, "old").unwrap();
    app.answer("");
    step(&mut app, &mut shown, &mut win, 5);
    assert!(ui::diff(&shown, &shown).is_empty());
}

#[test]
fn hostile_names_and_text_are_drawn_as_text_in_bounds() {
    let mut app = App::new();
    let long = "n".repeat(250);
    app.files.refresh(&mut app.os);
    app.answer(&format!("{long}\n\u{1b}[31mred.txt\nok.txt\n"));
    app.files.select(&mut app.os, "ok.txt").unwrap();
    let text: String = (0..200)
        .map(|i| format!("line {i} {}\u{1b}[0m\n", "y".repeat(300)))
        .collect();
    app.answer(&text);
    let tree = app.tree();
    // The escape name was left out of the list; the long one is cut.
    let labels: Vec<&str> = tree
        .iter()
        .filter_map(|e| match &e.widget {
            Widget::Label { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(labels.iter().all(|t| t.chars().all(|c| !c.is_control())));
    assert!(labels.iter().all(|t| t.chars().count() <= 200));
    assert!(!labels.iter().any(|t| t.contains("red.txt")));
    assert!(tree.iter().filter(|e| e.id >= ui::LINE).count() <= 26);
    // And uikit takes the whole tree.
    let _ = frame(&tree);
    assert!(matches!(
        app.files.preview(),
        Preview::Text { cut: true, .. }
    ));
    assert!(tree.iter().any(|e| e.id == ITEM));
}

#[test]
fn the_count_says_one_folder_and_one_file_in_the_singular() {
    let count = |listing: &str| {
        let mut app = App::new();
        app.files.refresh(&mut app.os);
        app.answer(listing);
        let tree = app.tree();
        match &tree.iter().find(|e| e.id == ui::STATUS).unwrap().widget {
            Widget::Label { text, .. } => text.clone(),
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(count(""), "0 folders, 0 files");
    assert_eq!(count("a.txt\n"), "0 folders, 1 file");
    assert_eq!(count("docs/\n"), "1 folder, 0 files");
    assert_eq!(count("docs/\na.txt\n"), "1 folder, 1 file");
    assert_eq!(count("a/\nb/\nc.txt\nd.txt\n"), "2 folders, 2 files");
}
