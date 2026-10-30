//! `kai:ui/view` and `on-ui` through a hand-written canonical-ABI guest: its
//! exports are thin trampolines into the view imports, so each test lays out the
//! arguments with `appabi` and reads the result area back.

extern crate std;

use crate::{Limits, Update, load_core};
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use appabi::{Core, Guest, Ty, Val};
use uikit::a11y::Action;
use uikit::geom::Point;
use uikit::input::Input;

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

/// Where the trampolines put a view call's result, and `on-ui` its last event.
const RET: u32 = 800;
const EVENT: u32 = 900;
const TITLE: u32 = 960;

const GUEST: &str = r#"(module
  (import "kai:ui/view@0.1.0" "open" (func $open (param i32 i32 i32 i32 i32)))
  (import "kai:ui/view@0.1.0" "put" (func $put (param i32 i32)))
  (import "kai:ui/view@0.1.0" "commit" (func $commit (param i32 i32)))
  (import "kai:ui/view@0.1.0" "close" (func $close (param i32 i32)))
  (memory (export "memory") 1)
  (global $top (mut i32) (i32.const 1024))
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
  (func (export "t-close") (param i32)
    (call $close (local.get 0) (i32.const 800)))
  (func (export "kai:ui/input@0.1.0#on-ui") (param i32 i32 i32 i32 i32 i32)
    (i32.store (i32.const 900) (local.get 0))
    (i32.store (i32.const 904) (local.get 1))
    (i32.store (i32.const 908) (local.get 2))
    (i32.store (i32.const 912) (local.get 3))
    (i32.store (i32.const 916) (local.get 4))
    (i32.store (i32.const 920) (local.get 5))
    (i32.store (i32.const 924) (i32.add (i32.load (i32.const 924)) (i32.const 1)))))"#;

fn guest() -> crate::CoreInstance {
    load_core(&wat::parse_str(GUEST).unwrap(), LIMITS, None).unwrap()
}

fn word(g: &mut crate::CoreInstance, at: u32) -> u32 {
    let b = g.read(at, 4).unwrap();
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// `(is_err, payload)` of `open`'s result area: the window id, or the error's case.
fn result(g: &mut crate::CoreInstance) -> (u8, u32) {
    let b = g.read(RET, 8).unwrap();
    (b[0], u32::from_le_bytes([b[4], b[5], b[6], b[7]]))
}

fn open(g: &mut crate::CoreInstance, title: &str) -> u32 {
    g.write(TITLE, title.as_bytes()).unwrap();
    let args = [TITLE, title.len() as u32, 200, 100].map(|v| Core::I32(v as i32));
    Guest::call(g, "t-open", &args).unwrap();
    let (err, id) = result(g);
    assert_eq!(err, 0, "open failed with case {id}");
    id
}

fn fit() -> Val {
    let fit = Val::Variant(0, None);
    let pad = Val::Record(vec![Val::U16(0); 4]);
    Val::Record(vec![fit.clone(), fit, pad, Val::U16(0)])
}

fn button(text: &str) -> Val {
    let fields = vec![Val::String(String::from(text)), Val::Option(None)];
    Val::Variant(2, Some(Box::new(Val::Record(fields))))
}

/// `put` with these arguments; the result area's `(is_err, case)`.
fn put(g: &mut crate::CoreInstance, window: u32, node: u32, widget: Val) -> Result<(u8, u8), ()> {
    put_raw(g, window, node, widget, None)
}

/// `put`, with the widget's case byte overwritten by `case` when given.
fn put_raw(
    g: &mut crate::CoreInstance,
    window: u32,
    node: u32,
    widget: Val,
    case: Option<u8>,
) -> Result<(u8, u8), ()> {
    let entry = crate::IMPORTS
        .iter()
        .find(|f| f.interface == "kai:ui/view" && f.name == "put")
        .unwrap();
    let tuple = Ty::Tuple((entry.params)());
    let at = g
        .realloc(0, 0, appabi::align(&tuple), appabi::size(&tuple))
        .unwrap();
    let args = Val::Tuple(vec![
        Val::U32(window),
        Val::U32(node),
        Val::U32(0),
        fit(),
        widget,
    ]);
    appabi::store_result(g, &args, &tuple, at).unwrap();
    if let Some(case) = case {
        let params = (entry.params)();
        let head = appabi::size(&Ty::Tuple(params[..4].to_vec()));
        let widget = appabi::align(&params[4]);
        g.write(at + head.div_ceil(widget) * widget, &[case])
            .unwrap();
    }
    Guest::call(g, "t-put", &[Core::I32(at as i32)]).map_err(|_| ())?;
    let b = g.read(RET, 2).unwrap();
    Ok((b[0], b[1]))
}

fn commit(g: &mut crate::CoreInstance, window: u32) {
    Guest::call(g, "t-commit", &[Core::I32(window as i32)]).unwrap();
    assert_eq!(result(g).0, 0);
}

#[test]
fn an_app_builds_a_window_and_comphost_gets_its_frame() {
    let mut g = guest();
    let w = open(&mut g, "Notes");
    assert_eq!(put(&mut g, w, 1, button("Go")), Ok((0, 0)));
    assert!(g.take_ui().is_empty(), "nothing shows before a commit");
    commit(&mut g, w);
    let updates = g.take_ui();
    let [Update::Frame { window, frame }] = updates.as_slice() else {
        panic!("{updates:?}");
    };
    assert_eq!(
        (*window, frame.title.as_str(), frame.size),
        (w, "Notes", (200, 100))
    );
    let b = frame.a11y.iter().find(|n| n.id == 1).unwrap();
    assert_eq!((b.role.name(), b.name.as_str()), ("button", "Go"));
    assert_eq!(g.ui_windows(), vec![w]);
}

#[test]
fn a_click_and_the_agent_reach_on_ui_as_the_same_event() {
    let mut g = guest();
    let w = open(&mut g, "Notes");
    put(&mut g, w, 1, button("Go")).unwrap();
    commit(&mut g, w);
    let frame = match g.take_ui().pop() {
        Some(Update::Frame { frame, .. }) => frame,
        other => panic!("{other:?}"),
    };
    let b = frame.a11y.iter().find(|n| n.id == 1).unwrap().bounds;
    let at = Point::new(b.x + b.w / 2, b.y + b.h / 2);
    g.ui_input(w, Input::Button { at, down: true }).unwrap();
    g.ui_input(w, Input::Button { at, down: false }).unwrap();
    assert!(g.last_call_by_user(), "a click is a person's input");
    // on-ui(window, pressed(1)): the window, case 0, node 1.
    let seen: Vec<u32> = (0..3).map(|i| word(&mut g, EVENT + 4 * i)).collect();
    assert_eq!((seen, word(&mut g, EVENT + 24)), (vec![w, 0, 1], 1));
    assert_eq!(g.ui_act(w, 1, Action::Click, None), Ok(Ok(())));
    assert_eq!(word(&mut g, EVENT + 24), 2);
    assert!(!g.last_call_by_user(), "the agent's action is not");
    assert_eq!(
        g.ui_act(w, 9, Action::Click, None),
        Ok(Err(crate::ActRefused::Node(uikit::ActError::NoNode)))
    );
    assert_eq!(
        g.ui_act(w + 1, 1, Action::Click, None),
        Ok(Err(crate::ActRefused::NoWindow))
    );
    // Close asked for: the window id rides in the event.
    g.ui_events(w, &[uikit::widget::UiEvent::CloseRequested])
        .unwrap();
    assert_eq!((word(&mut g, EVENT + 4), word(&mut g, EVENT + 8)), (3, w));
}

#[test]
fn errors_come_back_as_ui_errors_and_bad_values_trap() {
    let mut g = guest();
    // no-window is case 0.
    assert_eq!(put(&mut g, 7, 1, button("x")), Ok((1, 0)));
    let w = open(&mut g, "t");
    let label = Val::Variant(
        1,
        Some(Box::new(Val::Record(vec![
            Val::String("l".into()),
            Val::Enum(1),
            Val::Enum(0),
        ]))),
    );
    assert_eq!(put(&mut g, w, 1, label), Ok((0, 0)));
    // A widget case past the WIT's is refused while lifting: a trap.
    assert_eq!(put_raw(&mut g, w, 3, button("b"), Some(99)), Err(()));
    // Closing drops the window's pending frame and says it closed.
    commit(&mut g, w);
    Guest::call(&mut g, "t-close", &[Core::I32(w as i32)]).unwrap();
    assert_eq!(g.take_ui(), vec![Update::Closed(w)]);
    // Closed twice: an error of no-window, its case right after the tag.
    Guest::call(&mut g, "t-close", &[Core::I32(w as i32)]).unwrap();
    assert_eq!(g.read(RET, 2).unwrap(), [1, 0]);
}

#[test]
fn frames_coalesce_until_comphost_takes_them() {
    let mut g = guest();
    let w = open(&mut g, "t");
    commit(&mut g, w);
    put(&mut g, w, 1, button("a")).unwrap();
    commit(&mut g, w);
    let updates = g.take_ui();
    let [Update::Frame { frame, .. }] = updates.as_slice() else {
        panic!("{updates:?}");
    };
    assert!(frame.a11y.iter().any(|n| n.id == 1));
    assert!(!frame.damage.is_empty());
}

/// Notes with its window (components/notes-window), when it has been built for
/// wasm32: it opens its window when it starts, a click and the agent reach the
/// same note list, and asking to close closes it.
#[test]
fn the_notes_window_guest_shows_its_window_and_takes_clicks() {
    use livecore::Instance;
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../target/wasm32-unknown-unknown/component/comp_notes_window.wasm"
    );
    let Ok(wasm) = std::fs::read(path) else {
        std::eprintln!("skipped: build comp-notes-window for wasm32 first");
        return;
    };
    let mut g = load_core(&wasm, LIMITS, None).unwrap();
    // As the host loads it: storage readable, empty.
    g.view(&livecore::txn::Storage::new());
    assert_eq!(g.init(), Ok(0));
    let frame = |g: &mut crate::CoreInstance| match g.take_ui().pop() {
        Some(Update::Frame { window, frame }) => (window, frame),
        other => panic!("{other:?}"),
    };
    let (w, f) = frame(&mut g);
    assert_eq!(f.title, "Notes");
    let new = f.a11y.iter().find(|n| n.name == "New note").unwrap().id;
    assert_eq!(g.ui_act(w, new, Action::Click, None), Ok(Ok(())));
    let (_, f) = frame(&mut g);
    assert!(f.a11y.iter().any(|n| n.role.name() == "list"));
    let count = appabi::Func {
        export: String::from("app:notes/actions@1.0.0#count"),
        params: vec![],
        result: Some(Ty::Record(vec![
            (String::from("notes"), Ty::U32),
            (String::from("words"), Ty::U32),
        ])),
    };
    let tally = Instance::call(&mut g, &count, &[]).unwrap();
    assert_eq!(tally, Some(Val::Record(vec![Val::U32(1), Val::U32(0)])));
    // The agent's add redraws the window too.
    let add = appabi::Func {
        export: String::from("app:notes/actions@1.0.0#add"),
        params: vec![Ty::String],
        result: Some(Ty::Result(
            Some(Box::new(Ty::U32)),
            Some(Box::new(Ty::String)),
        )),
    };
    // As the host runs an action: in a call, whose writes reach storage.
    let storage = livecore::txn::Storage::new();
    g.begin(&storage, livecore::txn::Inputs::default());
    Instance::call(&mut g, &add, &[Val::String("milk".into())]).unwrap();
    let txn = g.finish();
    assert!(
        txn.writes.contains_key("notes/index"),
        "{:?}",
        txn.writes.keys()
    );
    let (_, f) = frame(&mut g);
    assert!(
        f.a11y.iter().any(|n| n.name.contains("milk")),
        "{:?}",
        f.a11y
    );
    g.ui_events(w, &[uikit::widget::UiEvent::CloseRequested])
        .unwrap();
    assert_eq!(g.take_ui(), vec![Update::Closed(w)]);
}

/// Keys as `desktop` posts them reach Notes' widgets through `uikit`'s routing
/// (the real guest, when built): Tab, Tab and Enter press "New note"; two more
/// Tabs reach the editor, and typing edits the note.
#[test]
fn keys_drive_the_notes_window_guest() {
    use livecore::Instance;
    use uikit::input::{Key, Mods};
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../target/wasm32-unknown-unknown/component/comp_notes_window.wasm"
    );
    let Ok(wasm) = std::fs::read(path) else {
        std::eprintln!("skipped: build comp-notes-window for wasm32 first");
        return;
    };
    let mut g = load_core(&wasm, LIMITS, None).unwrap();
    g.view(&livecore::txn::Storage::new());
    assert_eq!(g.init(), Ok(0));
    let w = match g.take_ui().pop() {
        Some(Update::Frame { window, .. }) => window,
        other => panic!("{other:?}"),
    };
    let press = |g: &mut crate::CoreInstance, key: Key, text: &str| {
        for down in [true, false] {
            let input = Input::Key {
                key,
                down,
                mods: Mods::default(),
                text: if down {
                    String::from(text)
                } else {
                    String::new()
                },
            };
            g.ui_input(w, input).unwrap();
        }
    };
    press(&mut g, Key::Tab, "");
    press(&mut g, Key::Tab, "");
    press(&mut g, Key::Enter, "");
    let get = appabi::Func {
        export: String::from("app:notes/actions@1.0.0#get"),
        params: vec![Ty::U32],
        result: Some(Ty::Option(Box::new(Ty::String))),
    };
    let note = |g: &mut crate::CoreInstance| Instance::call(g, &get, &[Val::U32(1)]).unwrap();
    assert_eq!(
        note(&mut g),
        Some(Val::Option(Some(Box::new(Val::String(String::new())))))
    );
    press(&mut g, Key::Tab, "");
    press(&mut g, Key::Tab, "");
    press(&mut g, Key::Other(0x0B), "h");
    press(&mut g, Key::Other(0x0C), "i");
    assert_eq!(
        note(&mut g),
        Some(Val::Option(Some(Box::new(Val::String(String::from("hi"))))))
    );
}

/// `set-agent-private` on window 1 of a host holding one open window.
fn keep_private(host: &mut crate::ui::UiHost, private: bool) -> Result<(), String> {
    let args = [Val::U32(1), Val::Bool(private)];
    crate::ui::run(host, "set-agent-private", &args).map(|done| {
        assert_eq!(done, Ok(None));
    })
}

#[test]
fn only_a_granted_app_keeps_a_window_from_the_agent() {
    let mut host = crate::ui::UiHost::default();
    let open = [Val::String("Settings".into()), Val::U16(320), Val::U16(200)];
    assert_eq!(crate::ui::run(&mut host, "open", &open), Ok(Ok(Some(1))));
    // Without the grant the call traps, and the window stays shared.
    let refused = keep_private(&mut host, true).unwrap_err();
    assert!(refused.contains("grant"), "{refused}");
    let frame = host
        .ui
        .window(1)
        .unwrap()
        .commit(&uikit::layout::FixedAdvance);
    assert!(!frame.agent_private);
    // With it, the flag shows at the next commit, and can be lifted again.
    host.may_be_private = true;
    keep_private(&mut host, true).unwrap();
    let frame = host
        .ui
        .window(1)
        .unwrap()
        .commit(&uikit::layout::FixedAdvance);
    assert!(frame.agent_private);
    keep_private(&mut host, false).unwrap();
    let frame = host
        .ui
        .window(1)
        .unwrap()
        .commit(&uikit::layout::FixedAdvance);
    assert!(!frame.agent_private);
}
