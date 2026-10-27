//! Notes' window: golden display lists of its views, drawn by `uikit` with
//! `FixedAdvance` in `look`'s token names, and the round trip from window events
//! to state. Set `UIKIT_BLESS=1` to rewrite the goldens, and review the diff.

use comp_notes::{Actions, Notes};
use comp_notes_ui::{self as ui, Change, EDITOR, Entry, LIST, NEW, SEARCH, View};
use compsdk::Component;
use std::path::PathBuf;
use uikit::layout::FixedAdvance as M;
use uikit::widget::{UiEvent, Value};
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

fn some_notes() -> Notes {
    let mut n = Notes::init();
    n.add("Groceries\nmilk and eggs").unwrap();
    n.add("Ideas for the desktop: a launcher").unwrap();
    n.add("Call the plumber about the kitchen sink on Tuesday morning")
        .unwrap();
    n
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
    let mut win = Window::new("Notes", w, h);
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
fn the_list_view() {
    let notes = some_notes();
    let f = frame(&ui::build(&notes, &View::default()));
    golden("list", &f.list.to_string());
    golden("list-a11y", &a11y(&f));
}

#[test]
fn the_editor_view() {
    let notes = some_notes();
    let view = View {
        search: String::new(),
        selected: Some(1),
    };
    let f = frame(&ui::build(&notes, &view));
    golden("editor", &f.list.to_string());
    golden("editor-a11y", &a11y(&f));
}

#[test]
fn the_search_view() {
    let notes = some_notes();
    let view = View {
        search: String::from("THE"),
        selected: None,
    };
    let f = frame(&ui::build(&notes, &view));
    golden("search", &f.list.to_string());
    golden("search-a11y", &a11y(&f));
}

#[test]
fn new_note_runs_the_declared_action_and_opens_the_note() {
    let mut notes = some_notes();
    let mut view = View::default();
    let tree = ui::build(&notes, &view);
    let new = tree.iter().find(|e| e.id == NEW).unwrap();
    assert!(
        matches!(&new.widget, uikit::widget::Widget::Button { action: Some(a), .. } if a == "notes_add")
    );
    assert!(ui::on_ui(&mut notes, &mut view, &UiEvent::Pressed(NEW)));
    assert_eq!(view.selected, Some(4));
    assert_eq!(notes.get(4).as_deref(), Some(""));
}

#[test]
fn typing_in_the_window_changes_the_state() {
    let mut notes = some_notes();
    let mut view = View::default();
    let old = ui::build(&notes, &view);
    let mut win = window(&old);
    win.commit(&M);
    // The user clicks the second note, then types in the editor.
    let picked = win
        .act(
            LIST,
            uikit::a11y::Action::SetValue,
            Some(Value::Index(1)),
            &M,
        )
        .unwrap();
    for e in &picked {
        ui::on_ui(&mut notes, &mut view, e);
    }
    assert_eq!(view.selected, Some(2));
    let tree = ui::build(&notes, &view);
    apply(&mut win, &ui::diff(&old, &tree));
    win.commit(&M);
    win.focus(EDITOR).unwrap();
    let typed = win.input(
        uikit::input::Input::Key {
            key: uikit::input::Key::Other(0x04),
            down: true,
            mods: Default::default(),
            text: String::from("!"),
        },
        &M,
    );
    for e in &typed {
        ui::on_ui(&mut notes, &mut view, e);
    }
    assert_eq!(
        notes.get(2).as_deref(),
        Some("Ideas for the desktop: a launcher!")
    );
    assert_eq!(notes.count().words, 4 + 6 + 10);
}

#[test]
fn search_filters_and_enter_opens_the_first_match() {
    let mut notes = some_notes();
    let mut view = View::default();
    assert!(ui::on_ui(
        &mut notes,
        &mut view,
        &UiEvent::Changed(SEARCH, Value::Text(String::from("milk")))
    ));
    assert_eq!(ui::shown(&notes, &view).len(), 1);
    assert!(ui::on_ui(
        &mut notes,
        &mut view,
        &UiEvent::Submitted(SEARCH)
    ));
    assert_eq!(view.selected, Some(1));
    view.search = String::from("nothing");
    assert!(!ui::on_ui(
        &mut notes,
        &mut view,
        &UiEvent::Submitted(SEARCH)
    ));
}

#[test]
fn the_diff_sends_only_what_changed_and_rebuilds_the_same_tree() {
    let mut notes = some_notes();
    let views = [
        View::default(),
        View {
            search: String::new(),
            selected: Some(3),
        },
        View {
            search: String::from("the"),
            selected: Some(3),
        },
        View {
            search: String::from("milk"),
            selected: None,
        },
        View::default(),
    ];
    let mut old: Vec<Entry> = Vec::new();
    let mut win = window(&old);
    for (i, view) in views.iter().enumerate() {
        if i == 4 {
            notes.add("one more").unwrap();
        }
        let new = ui::build(&notes, view);
        let changes = ui::diff(&old, &new);
        if i > 0 {
            assert!(changes.len() < new.len(), "step {i} sent the whole tree");
        }
        apply(&mut win, &changes);
        assert_eq!(win.commit(&M).list, frame(&new).list, "step {i}");
        old = new;
    }
    assert!(ui::diff(&old, &old).is_empty());
}

#[test]
fn list_labels_are_first_lines_cut_short() {
    assert_eq!(ui::title("  \nGroceries\nmilk"), "Groceries");
    assert_eq!(ui::title(""), "(empty)");
    let long = "x".repeat(50);
    assert_eq!(ui::title(&long).chars().count(), ui::ITEM_CHARS + 1);
}

#[test]
fn the_editor_refuses_text_past_the_state() {
    let mut notes = Notes::init();
    let id = notes.add("a").unwrap();
    assert!(ui::set_text(&mut notes, id, "a b"));
    assert_eq!(notes.count().words, 2);
    let huge = "y".repeat(compsdk::MAX_STATE);
    assert!(!ui::set_text(&mut notes, id, &huge));
    assert!(!ui::set_text(&mut notes, 99, "z"));
}
