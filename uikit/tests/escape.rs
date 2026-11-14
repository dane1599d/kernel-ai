//! The window's escape button (#892, `uikit::escape`): Esc presses it, the
//! mark is a key binding only, one per window, and only when nothing else
//! takes Esc.

mod common;

use common::s;
use uikit::Window;
use uikit::a11y::Action;
use uikit::escape::{ESCAPE, escape_action};
use uikit::input::{Input, Key, Mods};
use uikit::layout::FixedAdvance as M;
use uikit::widget::{Axis, CodeView, Layout, ROOT, UiEvent, Widget};

fn esc() -> Input {
    Input::Key {
        key: Key::Escape,
        down: true,
        mods: Mods::default(),
        text: String::new(),
    }
}

fn button(w: &mut Window, id: u32, text: &str, action: Option<String>) {
    w.put(
        id,
        1,
        Layout::FIT,
        Widget::Button {
            text: s(text),
            action,
        },
    )
    .unwrap();
}

/// A Terminal-shaped window: a column with Clear (a real action) and Stop.
fn window() -> Window {
    let mut w = Window::new("Terminal", 400, 200);
    w.put(1, ROOT, Layout::FILL, Widget::Stack { axis: Axis::Column })
        .unwrap();
    button(&mut w, 2, "Clear", Some(s("terminal_window_clear")));
    button(&mut w, 3, "Stop", Some(escape_action(None)));
    w.commit(&M);
    w
}

#[test]
fn esc_presses_the_escape_button_with_nothing_focused() {
    let mut w = window();
    assert_eq!(w.focus_node(), None);
    assert_eq!(w.input(esc(), &M), [UiEvent::Pressed(3)]);
    // With focus on another button, Esc still presses Stop, never the focus.
    w.focus(2).unwrap();
    assert_eq!(w.input(esc(), &M), [UiEvent::Pressed(3)]);
}

/// Point 1: the mark is a key binding only. The a11y tree's bound action
/// never carries it, and a marked button keeps its own action there.
#[test]
fn the_mark_never_reaches_the_bound_actions() {
    let mut w = window();
    button(
        &mut w,
        4,
        "Cancel",
        Some(escape_action(Some("notes_cancel"))),
    );
    let f = w.commit(&M);
    let bound = |id| f.a11y.iter().find(|n| n.id == id).unwrap().bound.clone();
    assert_eq!(bound(3), None);
    assert_eq!(bound(4).as_deref(), Some("notes_cancel"));
    assert!(
        f.a11y
            .iter()
            .all(|n| !n.bound.as_deref().is_some_and(|b| b.contains(ESCAPE)))
    );
    // The agent presses it as any button, with no action named for it.
    assert_eq!(
        w.act(3, Action::Click, None, &M),
        Ok(vec![UiEvent::Pressed(3)])
    );
}

/// Point 2: one escape button per window; with two marked, Esc presses the
/// first and the host is told once.
#[test]
fn two_marked_buttons_press_the_first_and_are_noted_once() {
    let mut w = window();
    button(&mut w, 4, "Cancel", Some(escape_action(None)));
    w.commit(&M);
    assert_eq!(w.input(esc(), &M), [UiEvent::Pressed(3)]);
    let notes = w.take_notes();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains("2 escape buttons"), "{notes:?}");
    w.input(esc(), &M);
    assert!(w.take_notes().is_empty(), "noted once");
}

/// Point 3: an open menu takes Esc first (it closes), and a focused code view
/// takes it as its own key. `desktop`'s card takes every key while it is up,
/// so no window sees Esc then (`deskcore::desk::ask`).
#[test]
fn a_menu_or_a_code_view_takes_esc_first() {
    let mut w = window();
    w.put(
        5,
        1,
        Layout::FIT,
        Widget::Menu {
            label: s("Sort"),
            items: vec![s("Name"), s("Size")],
        },
    )
    .unwrap();
    w.commit(&M);
    w.focus(5).unwrap();
    // Enter opens the menu; Esc closes it and presses nothing.
    w.input(
        Input::Key {
            key: Key::Enter,
            down: true,
            mods: Mods::default(),
            text: String::new(),
        },
        &M,
    );
    assert!(w.input(esc(), &M).is_empty());
    // Closed, the next Esc is the escape button's.
    assert_eq!(w.input(esc(), &M), [UiEvent::Pressed(3)]);

    w.put(
        6,
        1,
        Layout::FIT,
        Widget::Code(CodeView {
            label: s("lib.rs"),
            gutter_width: 4,
            rows: Vec::new(),
            cursor: None,
            selected: None,
        }),
    )
    .unwrap();
    w.commit(&M);
    w.focus(6).unwrap();
    let events = w.input(esc(), &M);
    assert!(
        matches!(events.as_slice(), [UiEvent::Key { node: 6, name, .. }] if name == "esc"),
        "{events:?}"
    );
}

#[test]
fn a_window_with_no_escape_button_ignores_esc_and_ctrl_esc_is_not_it() {
    let mut w = Window::new("Notes", 400, 200);
    w.put(1, ROOT, Layout::FILL, Widget::Stack { axis: Axis::Column })
        .unwrap();
    button(&mut w, 2, "New", Some(s("notes_add")));
    w.commit(&M);
    assert!(w.input(esc(), &M).is_empty());
    let mut w = window();
    let ctrl_esc = Input::Key {
        key: Key::Escape,
        down: true,
        mods: Mods(Mods::CTRL),
        text: String::new(),
    };
    assert!(w.input(ctrl_esc, &M).is_empty());
}
