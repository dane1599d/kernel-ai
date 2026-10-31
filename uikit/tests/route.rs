//! Input routing, the agent's actions, focus and damage.

mod common;

use common::{label, s};
use uikit::a11y::Action;
use uikit::geom::{Point, Rect};
use uikit::input::{Input, Key, Mods};
use uikit::layout::FixedAdvance as M;
use uikit::widget::{Axis, Layout, ROOT, UiEvent, Value, Widget};
use uikit::{ActError, Ui, Window};

fn key(k: Key) -> Input {
    Input::Key {
        key: k,
        down: true,
        mods: Mods::default(),
        text: String::new(),
    }
}

fn typed(t: &str) -> Input {
    Input::Key {
        key: Key::Other(0x04),
        down: true,
        mods: Mods::default(),
        text: s(t),
    }
}

fn centre(w: &mut Window, id: u32) -> Point {
    let f = w.commit(&M);
    let b = f.a11y.iter().find(|n| n.id == id).unwrap().bounds;
    Point::new(b.x + b.w / 2, b.y + b.h / 2)
}

fn click(w: &mut Window, at: Point) -> Vec<UiEvent> {
    let mut out = w.input(
        Input::Button {
            at,
            down: true,
            mods: Default::default(),
        },
        &M,
    );
    out.extend(w.input(
        Input::Button {
            at,
            down: false,
            mods: Default::default(),
        },
        &M,
    ));
    out
}

#[test]
fn a_click_and_the_agent_press_the_same_button() {
    let mut w = common::notes();
    let at = centre(&mut w, 3);
    assert_eq!(click(&mut w, at), [UiEvent::Pressed(3)]);
    assert_eq!(w.focus_node(), Some(3));
    assert_eq!(
        w.act(3, Action::Click, None, &M),
        Ok(vec![UiEvent::Pressed(3)])
    );
}

#[test]
fn releasing_off_the_button_does_not_press_it() {
    let mut w = common::notes();
    let at = centre(&mut w, 3);
    w.input(
        Input::Button {
            at,
            down: true,
            mods: Default::default(),
        },
        &M,
    );
    let out = w.input(
        Input::Button {
            at: Point::new(470, 290),
            down: false,
            mods: Default::default(),
        },
        &M,
    );
    assert!(out.is_empty());
}

#[test]
fn typing_edits_the_focused_field_at_its_caret() {
    let mut w = common::notes();
    w.commit(&M);
    w.focus(6).unwrap();
    for k in [Key::End, Key::Left] {
        assert!(w.input(key(k), &M).is_empty());
    }
    let out = w.input(typed("é"), &M);
    assert_eq!(out, [UiEvent::Changed(6, Value::Text(s("Grocerieés")))]);
    let out = w.input(key(Key::Backspace), &M);
    assert_eq!(out, [UiEvent::Changed(6, Value::Text(s("Groceries")))]);
    w.input(key(Key::Home), &M);
    let out = w.input(key(Key::Delete), &M);
    assert_eq!(out, [UiEvent::Changed(6, Value::Text(s("roceries")))]);
    assert_eq!(w.input(key(Key::Enter), &M), [UiEvent::Submitted(6)]);
    // Shortcuts are not text.
    let ctrl = Input::Key {
        key: Key::Other(0x06),
        down: true,
        mods: Mods(Mods::CTRL),
        text: s("c"),
    };
    assert!(w.input(ctrl, &M).is_empty());
}

#[test]
fn enter_in_a_text_area_is_a_new_line() {
    let mut w = common::notes();
    w.commit(&M);
    w.focus(7).unwrap();
    let out = w.input(key(Key::Enter), &M);
    assert_eq!(out, [UiEvent::Changed(7, Value::Text(s("milk\neggs\n")))]);
}

#[test]
fn a_click_in_a_field_moves_its_caret() {
    let mut w = common::notes();
    let f = w.commit(&M);
    let b = f.a11y.iter().find(|n| n.id == 6).unwrap().bounds;
    // The box is below the 20 px label; "Gro" is 24 px past the 10 px padding.
    let at = Point::new(b.x + 10 + 24, b.y + 30);
    click(&mut w, at);
    let out = w.input(typed("X"), &M);
    assert_eq!(out, [UiEvent::Changed(6, Value::Text(s("GroXceries")))]);
}

#[test]
fn the_field_refuses_text_past_its_max() {
    let mut w = Window::new("t", 300, 100);
    let mut f = common::field("Name", "abc", "");
    f.max_len = 3;
    w.put(1, ROOT, Layout::FIT, Widget::TextLine(f)).unwrap();
    w.commit(&M);
    w.focus(1).unwrap();
    assert!(w.input(typed("d"), &M).is_empty());
    assert_eq!(
        w.act(1, Action::SetValue, Some(Value::Text(s("abcd"))), &M),
        Err(ActError::BadValue)
    );
    assert_eq!(
        w.act(1, Action::SetValue, Some(Value::Text(s("xy"))), &M),
        Ok(vec![UiEvent::Changed(1, Value::Text(s("xy")))])
    );
}

#[test]
fn tab_moves_focus_in_tree_order_and_wraps() {
    let mut w = common::notes();
    w.commit(&M);
    let mut seen = Vec::new();
    for _ in 0..5 {
        w.input(key(Key::Tab), &M);
        seen.push(w.focus_node().unwrap());
    }
    assert_eq!(seen, [3, 4, 6, 7, 3]);
    let back = Input::Key {
        key: Key::Tab,
        down: true,
        mods: Mods(Mods::SHIFT),
        text: String::new(),
    };
    w.input(back, &M);
    assert_eq!(w.focus_node(), Some(7));
}

#[test]
fn lists_select_by_click_and_by_arrow() {
    let mut w = common::notes();
    let at = centre(&mut w, 11);
    assert_eq!(click(&mut w, at), [UiEvent::Changed(4, Value::Index(1))]);
    assert_eq!(w.focus_node(), Some(4));
    assert_eq!(
        w.input(key(Key::Up), &M),
        [UiEvent::Changed(4, Value::Index(0))]
    );
    assert!(w.input(key(Key::Up), &M).is_empty());
    assert_eq!(
        w.act(4, Action::SetValue, Some(Value::Index(9)), &M),
        Err(ActError::BadValue)
    );
    // A row's padding, left of its text, is part of the row.
    let item = w_rect(&mut w, 11);
    let pad = Point::new(item.x - 4, item.y + item.h / 2);
    assert_eq!(click(&mut w, pad), [UiEvent::Changed(4, Value::Index(1))]);
}

#[test]
fn toggles_sliders_tabs_and_menus() {
    let mut w = common::gallery();
    w.commit(&M);
    assert_eq!(
        w.act(3, Action::Click, None, &M),
        Ok(vec![UiEvent::Changed(3, Value::On(false))])
    );
    assert_eq!(
        w.act(4, Action::Increment, None, &M),
        Ok(vec![UiEvent::Changed(4, Value::Number(16))])
    );
    assert_eq!(
        w.act(4, Action::SetValue, Some(Value::Number(99)), &M),
        Ok(vec![UiEvent::Changed(4, Value::Number(30))])
    );
    w.focus(4).unwrap();
    assert_eq!(
        w.input(key(Key::Home), &M),
        [UiEvent::Changed(4, Value::Number(10))]
    );
    // Tabs: the page changes, and the other page leaves the agent's tree.
    assert_eq!(
        w.act(8, Action::SetValue, Some(Value::Index(1)), &M),
        Ok(vec![UiEvent::Changed(8, Value::Index(1))])
    );
    let f = w.commit(&M);
    assert!(f.a11y.iter().any(|n| n.id == 12) && !f.a11y.iter().any(|n| n.id == 9));
    // A menu opens on a click and a click on an item picks it.
    let at = centre(&mut w, 6);
    click(&mut w, at);
    let item = uikit::paint::menu_item(w_rect(&mut w, 6), 1);
    let out = click(&mut w, Point::new(item.x + 5, item.y + 5));
    assert_eq!(out, [UiEvent::Changed(6, Value::Index(1))]);
}

fn w_rect(w: &mut Window, id: u32) -> Rect {
    w.commit(&M)
        .a11y
        .iter()
        .find(|n| n.id == id)
        .unwrap()
        .bounds
}

#[test]
fn the_agent_cannot_do_what_a_node_does_not_offer() {
    let mut w = common::notes();
    w.commit(&M);
    assert_eq!(
        w.act(10, Action::Click, None, &M),
        Err(ActError::NotAllowed)
    );
    assert_eq!(w.act(99, Action::Focus, None, &M), Err(ActError::NoNode));
    assert_eq!(
        w.act(3, Action::SetValue, Some(Value::On(true)), &M),
        Err(ActError::NotAllowed)
    );
    assert_eq!(
        w.act(6, Action::SetValue, Some(Value::On(true)), &M),
        Err(ActError::BadValue)
    );
}

#[test]
fn the_wheel_scrolls_and_focus_scrolls_into_view() {
    let mut w = Window::new("s", 200, 100);
    w.put(1, ROOT, Layout::FILL, Widget::Scroll { axis: Axis::Column })
        .unwrap();
    w.put(2, 1, Layout::FILL, Widget::Stack { axis: Axis::Column })
        .unwrap();
    for i in 0..20 {
        w.put(
            10 + i,
            2,
            Layout::FIT,
            Widget::Button {
                text: s("b"),
                action: None,
            },
        )
        .unwrap();
    }
    w.commit(&M);
    w.input(
        Input::Wheel {
            at: Point::new(10, 10),
            dy: 1,
        },
        &M,
    );
    let f = w.commit(&M);
    // The first button scrolled out of view.
    assert!(
        f.a11y
            .iter()
            .find(|n| n.id == 10)
            .unwrap()
            .bounds
            .is_empty()
    );
    // Only the moved buttons and the scroll bar repaint.
    let window = Rect::new(0, 0, 200, 100);
    assert!(
        !f.damage.is_empty()
            && f.damage
                .iter()
                .all(|d| window.intersect(d) == *d && d.w < 200)
    );
    w.act(29, Action::Focus, None, &M).unwrap();
    let f = w.commit(&M);
    let b = f.a11y.iter().find(|n| n.id == 29).unwrap().bounds;
    assert!(b.y >= 0 && b.bottom() <= 100, "{b:?}");
}

#[test]
fn damage_covers_only_what_changed() {
    let mut w = common::notes();
    assert_eq!(w.commit(&M).damage, [Rect::new(0, 0, 480, 300)]);
    assert!(w.commit(&M).damage.is_empty());
    w.put(11, 4, common::wide(), label("Ideas!")).unwrap();
    let f = w.commit(&M);
    assert_eq!(f.damage.len(), 1);
    let d = f.damage[0];
    let item = f.a11y.iter().find(|n| n.id == 11).unwrap().bounds;
    assert!(d.overlaps(&item) && d.w < 200 && d.h <= 30, "{d:?}");
    w.resize(500, 300);
    assert_eq!(w.commit(&M).damage, [Rect::new(0, 0, 500, 300)]);
}

#[test]
fn removing_a_node_forgets_its_state() {
    let mut w = common::notes();
    w.commit(&M);
    w.focus(6).unwrap();
    w.remove(5).unwrap();
    assert_eq!(w.focus_node(), None);
    assert!(w.tree().get(7).is_none());
    assert!(w.input(typed("x"), &M).is_empty());
}

#[test]
fn an_app_has_at_most_eight_windows() {
    let mut ui = Ui::new();
    let ids: Vec<_> = (0..8).map(|_| ui.open("w", 10, 10).unwrap()).collect();
    assert_eq!(
        ui.open("w", 10, 10),
        Err(uikit::widget::UiError::TooManyWindows)
    );
    ui.close(ids[3]).unwrap();
    let again = ui.open("w", 10, 10).unwrap();
    assert!(!ids[..3].contains(&again) && ui.window(again).is_ok());
    assert_eq!(ui.close(999), Err(uikit::widget::UiError::NoWindow));
}
