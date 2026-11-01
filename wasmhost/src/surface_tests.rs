//! The `kai-surface 1` mapping on frames `uikit` really commits, checked against
//! what `surfabi` accepts.

use crate::surface::{self, BadRequest, Seat, wire_id};
use alloc::string::String;
use alloc::vec;
use surfabi::header::Event;
use surfabi::input::{InputEvent, Kind};
use surfabi::tree::{MAX_VALUE, Role as WireRole, Tree, Update, action, flag};
use uikit::a11y::Action;
use uikit::geom::{Point, Rect};
use uikit::input::{Input, Key, Mods};
use uikit::layout::FixedAdvance;
use uikit::widget::{Axis, Layout, ROOT, TextField, Value, Widget};
use uikit::{Frame, Window};

const TEXT: u32 = 3;
const SLIDER: u32 = 4;

fn frame(text: &str) -> Frame {
    let mut w = Window::new("Notes", 200, 100);
    w.put(1, ROOT, Layout::FIT, Widget::Stack { axis: Axis::Column })
        .unwrap();
    let button = Widget::Button {
        text: "Go".into(),
        action: None,
    };
    w.put(2, 1, Layout::FIT, button).unwrap();
    let field = TextField {
        label: "Body".into(),
        value: text.into(),
        placeholder: String::new(),
        max_len: 70_000,
    };
    w.put(TEXT, 1, Layout::FIT, Widget::TextArea(field))
        .unwrap();
    let slider = Widget::Slider {
        label: "Size".into(),
        min: 0,
        max: 10,
        step: 1,
        value: 5,
    };
    w.put(SLIDER, 1, Layout::FIT, slider).unwrap();
    w.focus(2).unwrap();
    w.commit(&FixedAdvance)
}

#[test]
fn a_frame_is_a_tree_update_desktop_accepts() {
    let f = frame("hi");
    let update = surface::tree_update(&f);
    assert_eq!((update.root, update.focus), (wire_id(ROOT), wire_id(2)));
    let bytes = update.encode();
    let decoded = Update::decode(&bytes).unwrap();
    let mut tree = Tree {
        root: 0,
        focus: 0,
        nodes: vec![],
    };
    tree.apply(decoded).unwrap();
    let root = tree.node(wire_id(ROOT)).unwrap();
    assert_eq!((root.role, root.name.as_str()), (WireRole::Window, "Notes"));
    let button = tree.node(wire_id(2)).unwrap();
    assert_eq!(button.role, WireRole::Button);
    assert_ne!(button.actions & action::CLICK, 0);
    assert_ne!(button.flags & flag::FOCUSABLE, 0);
    assert_eq!(
        tree.node(wire_id(TEXT)).unwrap().role,
        WireRole::MultilineTextInput
    );
}

#[test]
fn long_and_control_text_is_cut_to_what_the_tree_takes() {
    let long = "é".repeat(MAX_VALUE) + "\u{7}";
    let update = surface::tree_update(&frame(&long));
    let text = update.nodes.iter().find(|n| n.id == wire_id(TEXT)).unwrap();
    assert!(text.value.len() <= MAX_VALUE && text.value.chars().all(|c| c == 'é'));
    assert!(Update::decode(&update.encode()).is_ok());
}

#[test]
fn damage_is_in_buffer_px_and_cut_to_the_window() {
    let mut f = frame("");
    f.damage = vec![Rect::new(1, 1, 3, 3), Rect::new(190, 90, 50, 50)];
    let at = |scale| surface::damage(&f, scale);
    let r = |x, y, w, h| surfabi::Rect { x, y, w, h };
    assert_eq!(at(120), vec![r(1, 1, 3, 3), r(190, 90, 10, 10)]);
    // 1.5x: 1..4 becomes 1.5..6, grown to 1..6.
    assert_eq!(at(180)[0], r(1, 1, 5, 5));
    assert_eq!(at(240)[1], r(380, 180, 20, 20));
}

fn input(kind: Kind, code: u16, value: i32, text: &str) -> Event {
    let mut bytes = [0u8; 4];
    bytes[..text.len()].copy_from_slice(text.as_bytes());
    Event::Input {
        event: InputEvent {
            time_ns: 0,
            kind,
            code,
            value,
        },
        x: 10 << 8 | 0x80,
        y: 20 << 8,
        text: bytes,
        len: text.len() as u8,
    }
}

/// A key event, whose `x` carries the modifiers held with it (24.8).
fn key(code: u16, value: i32, text: &str, mods: u8) -> Event {
    match input(Kind::Key, code, value, text) {
        Event::Input {
            event,
            y,
            text,
            len,
            ..
        } => Event::Input {
            event,
            x: i32::from(mods) << 8,
            y,
            text,
            len,
        },
        other => other,
    }
}

#[test]
fn ring_events_become_uikit_input_with_the_seat_s_modifiers() {
    let mut seat = Seat::default();
    let at = Point::new(10, 20);
    assert_eq!(
        seat.input(&input(Kind::Abs, 0, 0, "")),
        Some(Input::PointerMove(at))
    );
    assert_eq!(
        seat.input(&input(Kind::Button, 1, 1, "")),
        Some(Input::Button {
            at,
            down: true,
            mods: Default::default()
        })
    );
    assert_eq!(seat.input(&input(Kind::Button, 2, 1, "")), None);
    assert_eq!(seat.input(&input(Kind::Screen, 0, 1, "")), None);
    assert_eq!(
        seat.input(&input(Kind::Wheel, 0, -2, "")),
        Some(Input::Wheel { at, dy: -2 })
    );
    assert_eq!(
        seat.input(&key(0x04, 1, "A", Mods::SHIFT)),
        Some(Input::Key {
            key: Key::Other(0x04),
            down: true,
            mods: Mods(Mods::SHIFT),
            text: "A".into(),
        })
    );
    // A button has the modifiers the seat last knew: the key's.
    assert!(matches!(
        seat.input(&input(Kind::Button, 1, 1, "")),
        Some(Input::Button {
            mods: Mods(Mods::SHIFT),
            ..
        })
    ));
    let done = Event::FrameDone { seq: 1, time_ns: 0 };
    assert_eq!(seat.input(&done), None);
}

#[test]
fn a_key_says_what_was_held_whatever_the_seat_s_message_and_when_it_comes() {
    // Ctrl-B as the ring has it: Ctrl down, B down, B up, Ctrl up, each with what was held.
    let mut seat = Seat::default();
    let mods_of = |input: Option<Input>| match input {
        Some(Input::Key { mods, .. }) => Some(mods),
        _ => None,
    };
    let ctrl = Some(Mods(Mods::CTRL));
    let none = Some(Mods::default());
    assert_eq!(mods_of(seat.input(&key(0xE0, 1, "", Mods::CTRL))), ctrl);
    // The seat's message comes late, by another road, and says Ctrl is up (it was
    // sent for the release that follows): the keys still carry Ctrl.
    seat.modifiers(0);
    assert_eq!(mods_of(seat.input(&key(0x05, 1, "", Mods::CTRL))), ctrl);
    assert_eq!(mods_of(seat.input(&key(0x05, 0, "", Mods::CTRL))), ctrl);
    seat.modifiers(u32::from(Mods::SHIFT));
    assert_eq!(mods_of(seat.input(&key(0xE0, 0, "", 0))), none);
    assert_eq!(mods_of(seat.input(&key(0x05, 1, "b", 0))), none);
    // Shift and Alt together, then one let go: each event is the set at its time.
    let both = Mods::SHIFT | Mods::ALT;
    assert_eq!(
        mods_of(seat.input(&key(0xE2, 1, "", both))),
        Some(Mods(both))
    );
    assert_eq!(
        mods_of(seat.input(&key(0xE1, 0, "", Mods::ALT))),
        Some(Mods(Mods::ALT))
    );
    // A button after them has the key's set.
    assert!(matches!(
        seat.input(&input(Kind::Button, 1, 1, "")),
        Some(Input::Button {
            mods: Mods(Mods::ALT),
            ..
        })
    ));
    // The seat's message is still how a button learns of a modifier held before the
    // window had the keys.
    seat.modifiers(u32::from(Mods::CTRL));
    assert!(matches!(
        seat.input(&input(Kind::Button, 1, 1, "")),
        Some(Input::Button {
            mods: Mods(Mods::CTRL),
            ..
        })
    ));
}

#[test]
fn action_requests_name_a_node_an_action_and_a_value_for_its_role() {
    let f = frame("");
    // Bit 0 is click; ids on the wire are one past uikit's.
    assert_eq!(
        surface::action_request(&f, wire_id(2), 0, ""),
        Ok((2, Action::Click, None))
    );
    assert_eq!(
        surface::action_request(&f, wire_id(SLIDER), 2, "7"),
        Ok((SLIDER, Action::SetValue, Some(Value::Number(7))))
    );
    assert_eq!(
        surface::action_request(&f, wire_id(TEXT), 2, "new"),
        Ok((TEXT, Action::SetValue, Some(Value::Text("new".into()))))
    );
    let bad = |node, act, v| surface::action_request(&f, node, act, v);
    assert_eq!(bad(0, 0, ""), Err(BadRequest::Node));
    assert_eq!(bad(99, 0, ""), Err(BadRequest::Node));
    assert_eq!(bad(wire_id(2), 5, ""), Err(BadRequest::Action));
    assert_eq!(bad(wire_id(2), 40, ""), Err(BadRequest::Action));
    assert_eq!(bad(wire_id(SLIDER), 2, "x"), Err(BadRequest::Value));
}
