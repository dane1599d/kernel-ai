//! `kai:code`: a code view put through a hand-written canonical-ABI guest, its
//! size told to the app once and again when it changes, and a key, a click and
//! the wheel handed to the app's `code-input` exports.

extern crate std;

use crate::{Limits, Update, load_core};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use appabi::{Core, Guest, Ty, Val};
use uikit::geom::Point;
use uikit::input::{Input, Key, Mods};

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

/// Where the trampolines put a call's result, and `code-input` the last event.
const RET: u32 = 800;
const KEY: u32 = 900;
const CLICK: u32 = 960;
const SCROLL: u32 = 1000;
const SIZE: u32 = 1020;
const TITLE: u32 = 1100;

const GUEST: &str = r#"(module
  (import "kai:ui/view@0.1.0" "open" (func $open (param i32 i32 i32 i32 i32)))
  (import "kai:ui/view@0.1.0" "commit" (func $commit (param i32 i32)))
  (import "kai:ui/view@0.1.0" "focus" (func $focus (param i32 i32 i32)))
  (import "kai:code/code@0.1.0" "put" (func $put (param i32 i32)))
  (memory (export "memory") 1)
  (global $top (mut i32) (i32.const 2048))
  (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
    (local $at i32)
    global.get $top
    local.get 2
    i32.add
    i32.const 1
    i32.sub
    i32.const 0
    local.get 2
    i32.sub
    i32.and
    local.tee $at
    local.get 3
    i32.add
    global.set $top
    local.get $at)
  (func (export "t-open") (param i32 i32 i32 i32)
    (call $open (local.get 0) (local.get 1) (local.get 2) (local.get 3) (i32.const 800)))
  (func (export "t-put") (param i32)
    (call $put (local.get 0) (i32.const 800)))
  (func (export "t-commit") (param i32)
    (call $commit (local.get 0) (i32.const 800)))
  (func (export "t-focus") (param i32 i32)
    (call $focus (local.get 0) (local.get 1) (i32.const 800)))
  (func (export "kai:code/code-input@0.1.0#on-key") (param i32 i32 i32 i32 i32 i32 i32)
    (i32.store (i32.const 900) (local.get 0))
    (i32.store (i32.const 904) (local.get 1))
    (i32.store (i32.const 908) (local.get 2))
    (i32.store (i32.const 912) (local.get 3))
    (i32.store (i32.const 916) (local.get 4))
    (i32.store (i32.const 920) (local.get 5))
    (i32.store (i32.const 924) (local.get 6))
    (i32.store (i32.const 928) (i32.add (i32.load (i32.const 928)) (i32.const 1))))
  (func (export "kai:code/code-input@0.1.0#on-click") (param i32 i32 i32 i32 i32)
    (i32.store (i32.const 960) (local.get 0))
    (i32.store (i32.const 964) (local.get 1))
    (i32.store (i32.const 968) (local.get 2))
    (i32.store (i32.const 972) (local.get 3))
    (i32.store (i32.const 976) (local.get 4)))
  (func (export "kai:code/code-input@0.1.0#on-scroll") (param i32 i32 i32)
    (i32.store (i32.const 1000) (local.get 0))
    (i32.store (i32.const 1004) (local.get 1))
    (i32.store (i32.const 1008) (local.get 2)))
  (func (export "kai:code/code-input@0.1.0#on-size") (param i32 i32 i32 i32)
    (i32.store (i32.const 1020) (local.get 0))
    (i32.store (i32.const 1024) (local.get 1))
    (i32.store (i32.const 1028) (local.get 2))
    (i32.store (i32.const 1032) (local.get 3))))"#;

fn guest() -> crate::CoreInstance {
    load_core(&wat::parse_str(GUEST).unwrap(), LIMITS, None).unwrap()
}

fn word(g: &mut crate::CoreInstance, at: u32) -> u32 {
    let b = g.read(at, 4).unwrap();
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn open(g: &mut crate::CoreInstance) -> u32 {
    g.write(TITLE, b"Editor").unwrap();
    let args = [TITLE, 6, 400, 300].map(|v| Core::I32(v as i32));
    Guest::call(g, "t-open", &args).unwrap();
    let b = g.read(RET, 8).unwrap();
    assert_eq!(b[0], 0, "open failed");
    u32::from_le_bytes([b[4], b[5], b[6], b[7]])
}

fn fill() -> Val {
    let fill = Val::Variant(1, Some(alloc::boxed::Box::new(Val::U16(1))));
    let pad = Val::Record(vec![Val::U16(0); 4]);
    Val::Record(vec![fill.clone(), fill, pad, Val::U16(0)])
}

fn span(start: u16, end: u16, link: bool) -> Val {
    Val::Record(vec![
        Val::U16(start),
        Val::U16(end),
        Val::Enum(3),
        Val::Bool(link),
    ])
}

fn row(text: &str, gutter: &str, spans: Vec<Val>) -> Val {
    Val::Record(vec![
        Val::String(text.into()),
        Val::String(gutter.into()),
        Val::List(spans),
    ])
}

fn view(rows: Vec<Val>) -> Val {
    Val::Record(vec![
        Val::String(String::from("notes 2: lib.rs")),
        Val::U16(4),
        Val::List(rows),
        Val::Option(Some(alloc::boxed::Box::new(Val::Tuple(vec![
            Val::U32(0),
            Val::U32(2),
        ])))),
        Val::Option(None),
    ])
}

fn fixed(w: u16, h: u16) -> Val {
    let fixed = |n| Val::Variant(2, Some(alloc::boxed::Box::new(Val::U16(n))));
    let pad = Val::Record(vec![Val::U16(0); 4]);
    Val::Record(vec![fixed(w), fixed(h), pad, Val::U16(0)])
}

/// `code.put` filling its parent; the result area's `(is_err, case)`.
fn put(g: &mut crate::CoreInstance, window: u32, node: u32, view: Val) -> (u8, u8) {
    put_in(g, window, node, fill(), view)
}

fn put_in(g: &mut crate::CoreInstance, window: u32, node: u32, layout: Val, view: Val) -> (u8, u8) {
    let entry = crate::IMPORTS
        .iter()
        .find(|f| f.interface == "kai:code/code" && f.name == "put")
        .unwrap();
    let tuple = Ty::Tuple((entry.params)());
    let at = g
        .realloc(0, 0, appabi::align(&tuple), appabi::size(&tuple))
        .unwrap();
    let args = Val::Tuple(vec![
        Val::U32(window),
        Val::U32(node),
        Val::U32(0),
        layout,
        view,
    ]);
    appabi::store_result(g, &args, &tuple, at).unwrap();
    Guest::call(g, "t-put", &[Core::I32(at as i32)]).unwrap();
    let b = g.read(RET, 2).unwrap();
    (b[0], b[1])
}

fn commit(g: &mut crate::CoreInstance, window: u32) {
    Guest::call(g, "t-commit", &[Core::I32(window as i32)]).unwrap();
    assert_eq!(g.read(RET, 1).unwrap()[0], 0, "commit failed");
}

fn sizes(updates: &[Update]) -> Vec<(u32, u32, u16, u16)> {
    updates
        .iter()
        .filter_map(|u| match u {
            Update::CodeSize {
                window,
                node,
                rows,
                cells,
            } => Some((*window, *node, *rows, *cells)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_code_view_is_put_drawn_and_its_size_told_once_and_again_when_it_changes() {
    let mut g = guest();
    let w = open(&mut g);
    let rows = vec![row("fn main() {", "  1", vec![span(0, 2, false)])];
    assert_eq!(put(&mut g, w, 1, view(rows.clone())), (0, 0));
    commit(&mut g, w);
    let updates = g.take_ui();
    let [first] = sizes(&updates)[..] else {
        panic!("{updates:?}");
    };
    assert_eq!((first.0, first.1), (w, 1));
    assert!(
        first.2 > 5 && first.3 > 20,
        "a 400 by 300 window: {first:?}"
    );
    let frame = updates
        .iter()
        .find_map(|u| match u {
            Update::Frame { frame, .. } => Some(frame),
            _ => None,
        })
        .unwrap();
    let node = frame.a11y.iter().find(|n| n.id == 1).unwrap();
    assert_eq!(node.name, "notes 2: lib.rs");
    assert_eq!(node.value.as_deref(), Some("fn main() {"));

    // Nothing changed: the same size is not told again.
    commit(&mut g, w);
    assert!(sizes(&g.take_ui()).is_empty());

    // The app puts the view again, smaller: told once more.
    assert_eq!(put_in(&mut g, w, 1, fixed(200, 100), view(rows)), (0, 0));
    commit(&mut g, w);
    let again = sizes(&g.take_ui());
    assert_eq!(again.len(), 1);
    assert!(again[0].2 < first.2 && again[0].3 < first.3);
}

#[test]
fn the_app_is_told_a_size_in_a_call_of_its_own() {
    let mut g = guest();
    g.code_size(7, 1, 24, 80).unwrap();
    assert_eq!(
        [SIZE, SIZE + 4, SIZE + 8, SIZE + 12].map(|at| word(&mut g, at)),
        [7, 1, 24, 80]
    );
}

#[test]
fn a_key_a_click_and_the_wheel_reach_the_apps_code_input() {
    let mut g = guest();
    let w = open(&mut g);
    assert_eq!(
        put(&mut g, w, 1, view(vec![row("abc", "", vec![])])),
        (0, 0)
    );
    commit(&mut g, w);
    let bounds = match g.take_ui().into_iter().find_map(|u| match u {
        Update::Frame { frame, .. } => Some(frame),
        _ => None,
    }) {
        Some(frame) => frame.a11y.iter().find(|n| n.id == 1).unwrap().bounds,
        None => panic!("no frame"),
    };
    Guest::call(&mut g, "t-focus", &[Core::I32(w as i32), Core::I32(1)]).unwrap();

    // A key with its text and modifiers: window, node, name, text, mods.
    let key = Input::Key {
        key: Key::Other(0x16),
        down: true,
        mods: Mods(2),
        text: String::new(),
    };
    g.ui_input(w, key).unwrap();
    assert!(g.last_call_by_user());
    let at = [KEY, KEY + 4, KEY + 12, KEY + 20].map(|a| word(&mut g, a));
    assert_eq!((at[0], at[1], at[3]), (w, 1, 0), "name is `s`, no text");
    let name_ptr = word(&mut g, KEY + 8);
    assert_eq!(g.read(name_ptr, 1).unwrap(), b"s");
    assert_eq!(word(&mut g, KEY + 24), 2, "ctrl");
    assert_eq!(word(&mut g, KEY + 28), 1, "one key so far");

    // A click inside the view: row, cell, mods.
    let b = bounds;
    let at = Point::new(b.x + 6 + 8 * 7, b.y + 6 + 2);
    let down = Input::Button {
        at,
        down: true,
        mods: Mods(2),
    };
    g.ui_input(w, down).unwrap();
    assert_eq!(word(&mut g, CLICK), w);
    assert_eq!(word(&mut g, CLICK + 4), 1);
    assert_eq!(word(&mut g, CLICK + 16), 2, "a Ctrl-click says so");

    // The wheel: three rows a step.
    let wheel = Input::Wheel {
        at: Point::new(b.x + 10, b.y + 10),
        dy: 2,
    };
    g.ui_input(w, wheel).unwrap();
    assert_eq!(word(&mut g, SCROLL + 8) as i32, 6);
}

#[test]
fn a_view_over_the_limits_is_refused_as_too_big() {
    let mut g = guest();
    let w = open(&mut g);
    let too_many = vec![row("x", "", vec![]); 201];
    assert_eq!(put(&mut g, w, 1, view(too_many)), (1, 3), "too-big");
    let long = row(&"y".repeat(401), "", vec![]);
    assert_eq!(put(&mut g, w, 1, view(vec![long])), (1, 3));
    // A span past the text's end or with no width is refused too.
    let backwards = row("abc", "", vec![span(2, 2, false)]);
    assert_eq!(put(&mut g, w, 1, view(vec![backwards])), (1, 3));
    // And a good one still goes in (an ok leaves the case byte as it was).
    let ok = put(&mut g, w, 1, view(vec![row("ok", "1", vec![])]));
    assert_eq!(ok.0, 0);
}

#[test]
fn an_app_without_code_input_exports_is_told_nothing() {
    let wat = GUEST.replace("kai:code/code-input@0.1.0#", "other#");
    let mut g = load_core(&wat::parse_str(&wat).unwrap(), LIMITS, None).unwrap();
    g.code_size(1, 1, 24, 80).unwrap();
    let w = open(&mut g);
    assert_eq!(
        put(&mut g, w, 1, view(vec![row("abc", "", vec![])])),
        (0, 0)
    );
}
