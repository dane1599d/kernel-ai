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

/// The first scroll bar thumb in node `id`'s ops: its first fill.
fn thumb_of(f: &uikit::Frame, id: u32) -> Rect {
    f.list
        .groups
        .iter()
        .filter(|g| g.node == id)
        .flat_map(|g| &g.ops)
        .find_map(|op| match op {
            uikit::list::Op::Fill { rect, .. } => Some(*rect),
            _ => None,
        })
        .unwrap()
}

/// A scroll view millions of px long (rows an app padded to the u16 limit),
/// scrolled to its end by focus, draws its thumb at the end of its bar: the
/// thumb's maths does not leave i32.
#[test]
fn a_scroll_view_millions_of_px_long_draws_its_thumb_at_the_end() {
    let mut w = Window::new("s", 400, 700);
    w.put(1, ROOT, Layout::FILL, Widget::Scroll { axis: Axis::Column })
        .unwrap();
    w.put(2, 1, Layout::FIT, Widget::Stack { axis: Axis::Column })
        .unwrap();
    let tall = Layout {
        padding: uikit::widget::Insets {
            top: u16::MAX,
            bottom: u16::MAX,
            ..Default::default()
        },
        ..Layout::FIT
    };
    for i in 0..40 {
        w.put(10 + i, 2, tall, Widget::Stack { axis: Axis::Column })
            .unwrap();
    }
    let end = Widget::Button {
        text: s("end"),
        action: None,
    };
    w.put(100, 2, Layout::FIT, end).unwrap();
    w.commit(&M);
    w.act(100, Action::Focus, None, &M).unwrap();
    let f = w.commit(&M);
    let thumb = thumb_of(&f, 1);
    let inside = Rect::new(0, 0, 400, 700).intersect(&thumb) == thumb;
    assert!(inside && thumb.h >= 16, "{thumb:?}");
    assert!(thumb.y > 600, "not at the end of the bar: {thumb:?}");
}

/// A list 65,535 px tall whose rows are longer still draws its thumb inside its
/// frame: `view * view` does not leave i32.
#[test]
fn a_list_65535_px_tall_draws_its_thumb_inside_its_frame() {
    let mut w = Window::new("s", 400, 700);
    let fixed = Layout {
        width: uikit::widget::Size::Fill(1),
        height: uikit::widget::Size::Fixed(u16::MAX),
        ..Layout::FIT
    };
    let list = Widget::ItemList {
        label: s("rows"),
        selected: None,
    };
    w.put(1, ROOT, fixed, list).unwrap();
    let row = Layout {
        padding: uikit::widget::Insets {
            top: u16::MAX,
            ..Default::default()
        },
        ..Layout::FIT
    };
    for i in 0..2 {
        w.put(10 + i, 1, row, Widget::Stack { axis: Axis::Column })
            .unwrap();
    }
    let f = w.commit(&M);
    let thumb = thumb_of(&f, 1);
    let inside = Rect::new(0, 0, 400, i32::from(u16::MAX)).intersect(&thumb) == thumb;
    assert!(inside && thumb.h > 16, "{thumb:?}");
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

/// A table shorter than its rows scrolls to keep the selection in view, and a
/// click picks the row drawn under it, not the one that would be there unscrolled.
#[test]
fn a_click_in_a_scrolled_table_picks_the_row_it_shows() {
    use uikit::style::metric::ROW_H;
    let mut w = Window::new("Monitor", 300, (4 * ROW_H + 2) as u16);
    let rows = (0..10).map(|i| vec![format!("p{i}")]).collect();
    w.put(
        1,
        ROOT,
        Layout::FILL,
        Widget::Table {
            label: s("Processes"),
            columns: vec![s("Name")],
            rows,
            selected: Some(9),
        },
    )
    .unwrap();
    let b = w.commit(&M).a11y.iter().find(|n| n.id == 1).unwrap().bounds;
    // Rows 7, 8 and 9 show; the first one under the heading is row 7.
    let at = Point::new(b.x + 4, b.y + 1 + ROW_H + ROW_H / 2);
    assert_eq!(click(&mut w, at), [UiEvent::Changed(1, Value::Index(7))]);
}

/// The founder's second test (backlog item 5): a window that takes the
/// keyboard with nothing focused focuses a node that takes text, so the
/// first key is not dropped; one that has a focused node keeps it.
#[test]
fn a_window_taking_the_keyboard_focuses_a_text_node_if_none_has_focus() {
    let mut w = common::notes();
    assert_eq!(w.focus_node(), None);
    w.input(Input::FocusIn, &M);
    assert_eq!(w.focus_node(), Some(6), "the title, the first text node");
    let out = w.input(typed("x"), &M);
    assert!(
        out.iter().any(|e| matches!(e, UiEvent::Changed(6, _))),
        "{out:?}"
    );
    // A node already focused keeps the focus across a focus out and in.
    w.focus(3).unwrap();
    w.input(Input::FocusOut, &M);
    w.input(Input::FocusIn, &M);
    assert_eq!(w.focus_node(), Some(3));
}

/// #872's review: a window with no node that takes text focuses nothing on
/// taking the keyboard, never a button a stray Space or Enter would press.
#[test]
fn a_window_with_only_buttons_focuses_nothing() {
    let mut w = Window::new("Buttons", 200, 100);
    w.put(1, ROOT, Layout::FILL, Widget::Stack { axis: Axis::Column })
        .unwrap();
    w.put(
        2,
        1,
        Layout::FIT,
        Widget::Button {
            text: s("Clear"),
            action: None,
        },
    )
    .unwrap();
    w.input(Input::FocusIn, &M);
    assert_eq!(w.focus_node(), None);
}

/// A list of `n` rows in a window 120 px tall, ids 10.. for its rows.
fn list(n: u32) -> Window {
    let mut w = Window::new("l", 200, 120);
    w.put(
        1,
        ROOT,
        Layout::FILL,
        Widget::ItemList {
            label: s("Notes"),
            selected: None,
        },
    )
    .unwrap();
    for i in 0..n {
        w.put(10 + i, 1, Layout::FIT, label(&format!("row {i}")))
            .unwrap();
    }
    w
}

fn wheel(w: &mut Window, at: Point, dy: i32) {
    w.input(Input::Wheel { at, dy }, &M);
}

fn bounds(w: &mut Window, id: u32) -> Rect {
    w.commit(&M)
        .a11y
        .iter()
        .find(|n| n.id == id)
        .unwrap()
        .bounds
}

#[test]
fn the_wheel_scrolls_a_list_longer_than_its_box_and_stops_at_its_ends() {
    let mut w = list(20);
    // The third row, which a step moves up and does not take out of the box.
    let first = bounds(&mut w, 12);
    wheel(&mut w, Point::new(20, 20), 1);
    let moved = bounds(&mut w, 12);
    assert_eq!(moved.y, first.y - uikit::style::metric::WHEEL_STEP);
    // Up from the top does nothing; far down stops with the last row inside.
    wheel(&mut w, Point::new(20, 20), -5);
    assert_eq!(bounds(&mut w, 12), first);
    wheel(&mut w, Point::new(20, 20), 100);
    let last = bounds(&mut w, 29);
    assert!(last.bottom() < 120, "{last:?}");
    assert!(last.bottom() >= 120 - 3, "{last:?}");
    wheel(&mut w, Point::new(20, 20), 1);
    assert_eq!(bounds(&mut w, 29), last);
}

#[test]
fn a_short_list_does_not_move_and_a_click_hits_the_row_under_it_once_scrolled() {
    let mut w = list(2);
    let before = bounds(&mut w, 11);
    wheel(&mut w, Point::new(20, 20), 3);
    assert_eq!(bounds(&mut w, 11), before);
    let mut w = list(20);
    wheel(&mut w, Point::new(20, 20), 2);
    let row = bounds(&mut w, 15);
    let at = Point::new(row.x + 4, row.y + row.h / 2);
    let heard = click(&mut w, at);
    assert!(
        heard.contains(&UiEvent::Changed(1, Value::Index(5))),
        "{heard:?}"
    );
}

#[test]
fn the_keys_keep_the_selected_row_in_view_of_a_list() {
    let mut w = list(20);
    w.act(1, Action::Focus, None, &M).unwrap();
    for _ in 0..12 {
        w.input(key(Key::Down), &M);
    }
    // Twelve Downs from nothing selected: the twelfth row.
    let row = bounds(&mut w, 21);
    assert!(row.y >= 1 && row.bottom() <= 119, "{row:?}");
    let first = bounds(&mut w, 10);
    assert!(first.is_empty() || first.y < 0, "{first:?}");
}

fn field(w: &mut Window, value: &str) {
    let f = uikit::widget::TextField {
        label: s("Name"),
        value: s(value),
        placeholder: String::new(),
        max_len: 4000,
    };
    w.put(2, ROOT, Layout::FIT, Widget::TextLine(f)).unwrap();
}

/// A caret kept as a byte offset must not stay inside a multi-byte character
/// when the app puts a new value under it: the next key used to slice there and
/// panic the host.
#[test]
fn a_put_that_leaves_the_caret_inside_a_character_does_not_break_the_next_key() {
    let mut w = Window::new("t", 200, 100);
    field(&mut w, "");
    w.commit(&M);
    w.act(2, Action::Focus, None, &M).unwrap();
    w.input(typed("a"), &M);
    w.input(typed("b"), &M);
    // The caret is at byte 2; the app answers with a value where byte 2 is inside "é".
    for value in ["né", "你好", "✓ sent", ""] {
        field(&mut w, value);
        w.commit(&M);
        w.input(typed("x"), &M);
        w.input(key(Key::Left), &M);
        w.input(key(Key::Backspace), &M);
        w.input(key(Key::Delete), &M);
        w.input(key(Key::End), &M);
        // Back to the byte-2 caret for the next value.
        w.input(key(Key::Home), &M);
        w.input(key(Key::Right), &M);
        w.input(key(Key::Right), &M);
    }
}

/// Typed text counts in the window's byte total, so removing the field later
/// takes off what it weighs and no more: the total never wraps, and a put after
/// it is not refused as too big.
#[test]
fn typing_then_removing_a_field_leaves_the_window_able_to_take_more() {
    let mut w = Window::new("t", 400, 200);
    w.put(1, ROOT, Layout::FILL, Widget::Stack { axis: Axis::Column })
        .unwrap();
    let f = uikit::widget::TextField {
        label: s("Line"),
        value: String::new(),
        placeholder: String::new(),
        max_len: 4000,
    };
    w.put(2, 1, Layout::FIT, Widget::TextLine(f)).unwrap();
    w.commit(&M);
    w.act(2, Action::Focus, None, &M).unwrap();
    for _ in 0..2000 {
        w.input(typed("x"), &M);
    }
    w.remove(2).unwrap();
    // The count is back where an empty window has it; one more put fits.
    w.put(3, 1, Layout::FIT, label("Ready")).unwrap();
    w.commit(&M);
    // And the same after a replace and a second removal.
    let f = uikit::widget::TextField {
        label: s("Line"),
        value: s("again"),
        placeholder: String::new(),
        max_len: 4000,
    };
    w.put(2, 1, Layout::FIT, Widget::TextLine(f)).unwrap();
    w.remove(2).unwrap();
    w.remove(3).unwrap();
    w.put(4, 1, Layout::FIT, label("Next")).unwrap();
}

/// Values the host accepts that would make `clamp` panic on a person's key or
/// click: tabs with no labels, a slider whose min is over its max.
#[test]
fn a_key_or_click_on_empty_tabs_or_an_inverted_slider_does_nothing_harmful() {
    let mut w = Window::new("t", 300, 120);
    w.put(1, ROOT, Layout::FILL, Widget::Stack { axis: Axis::Column })
        .unwrap();
    w.put(
        2,
        1,
        Layout::FIT,
        Widget::Tabs {
            labels: Vec::new(),
            selected: 0,
        },
    )
    .unwrap();
    w.put(
        3,
        1,
        Layout::FIT,
        Widget::Slider {
            label: s("Size"),
            min: 10,
            max: 0,
            step: 1,
            value: 5,
        },
    )
    .unwrap();
    w.commit(&M);
    w.act(2, Action::Focus, None, &M).unwrap();
    w.input(key(Key::Left), &M);
    w.input(key(Key::Right), &M);
    let at = centre(&mut w, 3);
    click(&mut w, at);
    w.input(Input::PointerMove(Point::new(at.x + 5, at.y)), &M);
}
