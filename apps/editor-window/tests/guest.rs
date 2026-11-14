//! The editor component as built for wasm32, run in `wasmhost` with no OS: it
//! opens its window when it starts, opens a file when the agent says so, draws
//! the service's answers in its code view, turns keys into requests for the edit
//! service and marks which of them a person's key or click caused. It is skipped
//! when `cargo build -p comp-editor-window --target wasm32-unknown-unknown
//! --profile component` has not run (CI runs it first).

use appabi::{Func, Ty, Val};
use comp_editor_ui::{CANCEL, CODE, DISCARD, SAVE};
use comp_editor_window::keys::CTRL;
use livecore::Instance;
use livecore::txn::{Inputs, Storage};
use uikit::widget::UiEvent;
use wasmhost::edit::MAX_QUEUED;
use wasmhost::{CoreInstance, FileOp, Grants, Limits, Update, load_core, load_core_granted};

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

fn wasm() -> Option<Vec<u8>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/wasm32-unknown-unknown/component/comp_editor_window.wasm"
    );
    let wasm = std::fs::read(path).ok();
    if wasm.is_none() {
        eprintln!("skipped: build comp-editor-window for wasm32 first");
    }
    wasm
}

fn granted() -> Grants {
    Grants {
        edit: true,
        files: true,
        ..Grants::default()
    }
}

/// More room than the default for the call's fuel and memory, for a file of the
/// largest size the editor takes: opening one costs far more than the default allows.
const BIG: Limits = Limits {
    fuel_call: 2_000_000_000,
    fuel_life: 20_000_000_000,
    memory_bytes: 16 * 1024 * 1024,
};

fn start(wasm: &[u8]) -> CoreInstance {
    start_with(wasm, LIMITS)
}

fn start_with(wasm: &[u8], limits: Limits) -> CoreInstance {
    let mut g = load_core_granted(wasm, limits, None, granted()).unwrap();
    g.view(&Storage::new());
    assert_eq!(g.init(), Ok(0));
    g
}

fn load() -> Option<CoreInstance> {
    wasm().map(|w| start(&w))
}

/// Runs `f` as the host runs a call.
fn call<R>(g: &mut CoreInstance, f: impl FnOnce(&mut CoreInstance) -> R) -> R {
    g.begin(&Storage::new(), Inputs::default());
    let out = f(g);
    g.finish();
    out
}

fn frame(g: &mut CoreInstance) -> (u32, uikit::Frame) {
    match g.take_ui().pop() {
        Some(Update::Frame { window, frame }) => (window, frame),
        other => panic!("{other:?}"),
    }
}

/// What the app has sent the edit service since the last look, as
/// `(ticket, message, by a person)`.
fn sent(g: &mut CoreInstance) -> Vec<(u32, String, bool)> {
    g.take_edits()
        .into_iter()
        .map(|r| (r.ticket, String::from_utf8(r.message).unwrap(), r.user))
        .collect()
}

fn answer(g: &mut CoreInstance, ticket: u32, outcome: Result<&str, &str>) {
    let outcome = outcome.map(|t| t.as_bytes().to_vec()).map_err(String::from);
    call(g, |g| g.edit_reply(ticket, outcome)).unwrap();
}

fn open_action() -> Func {
    Func {
        export: String::from("app:editor/actions@1.0.0#open"),
        params: vec![Ty::String, Ty::String, Ty::U32],
        result: Some(Ty::Result(
            Some(Box::new(Ty::String)),
            Some(Box::new(Ty::String)),
        )),
    }
}

fn status_action() -> Func {
    Func {
        export: String::from("app:editor/actions@1.0.0#status"),
        params: vec![],
        result: Some(Ty::String),
    }
}

fn open(g: &mut CoreInstance, component: &str, path: &str, line: u32) -> Result<String, String> {
    let r = call(g, |g| {
        Instance::call(
            g,
            &open_action(),
            &[
                Val::String(component.into()),
                Val::String(path.into()),
                Val::U32(line),
            ],
        )
    });
    match r.unwrap().unwrap() {
        Val::Result(Ok(Some(v))) => match *v {
            Val::String(s) => Ok(s),
            other => panic!("{other:?}"),
        },
        Val::Result(Err(Some(v))) => match *v {
            Val::String(s) => Err(s),
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

fn text_of(f: &uikit::Frame) -> String {
    f.a11y
        .iter()
        .map(|n| format!("{} {}", n.name, n.value.clone().unwrap_or_default()))
        .collect::<Vec<_>>()
        .join("\n")
}

const VIEW: &str = "ok view notes 2 src/lib.rs 3 2 -\n1:fn main() {\n2:    run();\n3:}";
const FILES: &str = "ok 1 files; * changed, > current\n > src/lib.rs (3 lines)";

/// The app opened on `src/lib.rs`, its three requests answered.
fn opened(g: &mut CoreInstance) -> u32 {
    let (w, _) = frame(g);
    open(g, "notes", "src/lib.rs", 2).unwrap();
    let asked = sent(g);
    assert_eq!(asked.len(), 1, "{asked:?}");
    answer(g, asked[0].0, Ok("ok opened notes 2"));
    for _ in 0..8 {
        let asked = sent(g);
        let Some((ticket, message, _)) = asked.first().cloned() else {
            break;
        };
        let reply = if message == "cmd files" {
            "ok 1 files; * changed, > current\n > src/lib.rs (3 lines)"
        } else {
            VIEW
        };
        answer(g, ticket, Ok(reply));
    }
    g.take_ui();
    w
}

#[test]
fn without_the_grant_the_app_does_not_start() {
    let Some(wasm) = wasm() else { return };
    let fault = match load_core(&wasm, LIMITS, None) {
        Err(fault) => fault,
        Ok(_) => panic!("it linked without the grant"),
    };
    assert_eq!(fault.reason, livecore::Reason::Link);
    assert!(fault.detail.contains("grant"), "{}", fault.detail);
    // Each granted interface is needed on its own.
    for (grants, missing) in [
        (
            Grants {
                edit: true,
                ..Grants::default()
            },
            "files grant",
        ),
        (
            Grants {
                files: true,
                ..Grants::default()
            },
            "edit grant",
        ),
    ] {
        let fault = match load_core_granted(&wasm, LIMITS, None, grants) {
            Err(fault) => fault,
            Ok(_) => panic!("it linked without the {missing}"),
        };
        assert_eq!(fault.reason, livecore::Reason::Link);
        assert!(fault.detail.contains(missing), "{}", fault.detail);
    }
}

#[test]
fn it_opens_an_empty_window_when_it_starts_and_asks_for_nothing() {
    let Some(mut g) = load() else { return };
    let (_, f) = frame(&mut g);
    assert_eq!(f.title, "Editor");
    assert!(sent(&mut g).is_empty());
}

#[test]
fn the_agent_opens_a_file_and_the_answers_fill_the_code_view() {
    let Some(mut g) = load() else { return };
    let (w, _) = frame(&mut g);
    let said = open(&mut g, "notes", "src/lib.rs", 2).unwrap();
    assert!(said.contains("src/lib.rs:2"), "{said}");
    let asked = sent(&mut g);
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].1, "open notes at src/lib.rs:2");
    // The agent's action is not a person's key.
    assert!(!asked[0].2);
    answer(&mut g, asked[0].0, Ok("ok opened notes 2"));
    // Then the file's size, its list and the lines.
    for _ in 0..8 {
        let asked = sent(&mut g);
        let Some((ticket, message, _)) = asked.first().cloned() else {
            break;
        };
        let reply = if message == "cmd files" {
            "ok 1 files; * changed, > current\n > src/lib.rs (3 lines)"
        } else {
            VIEW
        };
        answer(&mut g, ticket, Ok(reply));
    }
    let (window, f) = frame(&mut g);
    assert_eq!(window, w);
    let text = text_of(&f);
    assert!(text.contains("fn main() {"), "{text}");
    assert!(text.contains("src/lib.rs"), "{text}");
}

#[test]
fn a_bad_place_is_an_answer_not_a_request() {
    let Some(mut g) = load() else { return };
    frame(&mut g);
    assert!(open(&mut g, "notes", "/etc/passwd", 1).is_err());
    assert!(open(&mut g, "", "src/lib.rs", 1).is_err());
    assert!(open(&mut g, "notes", "src/lib.rs", 0).is_err());
    assert!(sent(&mut g).is_empty());
}

/// Ctrl-B, and the keys that type `x` first, as events from the code view.
fn key(name: &str, text: &str, mods: u8) -> UiEvent {
    UiEvent::Key {
        node: CODE,
        name: name.into(),
        text: text.into(),
        mods,
    }
}

/// Answers each request the app has out, to the end of what it sends in answer,
/// and gives every message with whether it was a person's.
fn chain(g: &mut CoreInstance) -> Vec<(String, bool)> {
    let mut all = Vec::new();
    for _ in 0..12 {
        let asked = sent(g);
        let Some((ticket, message, user)) = asked.first().cloned() else {
            break;
        };
        all.push((message.clone(), user));
        let reply = if message == "cmd files" {
            FILES
        } else if message == "cmd build" {
            "ok building e1"
        } else if message.starts_with("cmd view") {
            VIEW
        } else {
            "ok"
        };
        answer(g, ticket, Ok(reply));
    }
    all
}

#[test]
fn only_a_key_a_person_pressed_is_marked_as_theirs() {
    let Some(wasm) = wasm() else { return };
    let mut g = start(&wasm);
    let w = opened(&mut g);
    let build = key("b", "", CTRL);
    call(&mut g, |g| {
        g.ui_events_user(w, std::slice::from_ref(&build))
    })
    .unwrap();
    let real = sent(&mut g);
    assert_eq!(real.len(), 1, "{real:?}");
    assert_eq!((real[0].1.as_str(), real[0].2), ("cmd build", true));
    // The same key from anything else, such as the agent's `ui_act`.
    let mut g = start(&wasm);
    let w = opened(&mut g);
    call(&mut g, |g| g.ui_events(w, &[build])).unwrap();
    let synthetic = sent(&mut g);
    assert_eq!(synthetic.len(), 1, "{synthetic:?}");
    assert_eq!(
        (synthetic[0].1.as_str(), synthetic[0].2),
        ("cmd build", false)
    );
}

#[test]
fn a_build_asked_for_behind_an_edit_is_still_the_persons() {
    let Some(wasm) = wasm() else { return };
    let mut g = start(&wasm);
    let w = opened(&mut g);
    // Type, then Ctrl-B: the edit goes first and the build waits for its answer.
    call(&mut g, |g| g.ui_events_user(w, &[key("x", "x", 0)])).unwrap();
    call(&mut g, |g| g.ui_events_user(w, &[key("b", "", CTRL)])).unwrap();
    let sent_by_person = chain(&mut g);
    let kinds: Vec<&str> = sent_by_person.iter().map(|(m, _)| m.as_str()).collect();
    assert!(kinds[0].starts_with("cmd c "), "{kinds:?}");
    assert!(kinds.contains(&"cmd build"), "{kinds:?}");
    assert!(
        sent_by_person.iter().all(|(_, user)| *user),
        "{sent_by_person:?}"
    );

    // The same keys as the agent's events: none of it is a person's.
    let mut g = start(&wasm);
    let w = opened(&mut g);
    call(&mut g, |g| g.ui_events(w, &[key("x", "x", 0)])).unwrap();
    call(&mut g, |g| g.ui_events(w, &[key("b", "", CTRL)])).unwrap();
    let by_agent = chain(&mut g);
    assert!(
        by_agent.iter().any(|(m, _)| m == "cmd build"),
        "{by_agent:?}"
    );
    assert!(by_agent.iter().all(|(_, user)| !*user), "{by_agent:?}");
}

#[test]
fn a_failed_exchange_is_told_to_the_person_in_the_status() {
    let Some(mut g) = load() else { return };
    let w = opened(&mut g);
    let key = UiEvent::Key {
        node: CODE,
        name: "b".into(),
        text: String::new(),
        mods: CTRL,
    };
    call(&mut g, |g| g.ui_events_user(w, &[key])).unwrap();
    let asked = sent(&mut g);
    answer(&mut g, asked[0].0, Err("a build needs a key or a click"));
    let text = text_of(&frame(&mut g).1);
    assert!(text.contains("a build needs a key or a click"), "{text}");
}

#[test]
fn an_answer_for_a_ticket_it_did_not_ask_changes_nothing() {
    let Some(mut g) = load() else { return };
    opened(&mut g);
    answer(&mut g, 4242, Ok(VIEW));
    answer(&mut g, 0, Err("no"));
    assert!(sent(&mut g).is_empty());
}

#[test]
fn the_code_view_size_decides_how_many_lines_it_asks_for() {
    let Some(mut g) = load() else { return };
    let w = opened(&mut g);
    call(&mut g, |g| g.code_size(w, CODE, 2, 40)).unwrap();
    let asked = sent(&mut g);
    assert!(
        asked.iter().all(|(_, m, _)| m.starts_with("cmd view ")),
        "{asked:?}"
    );
    assert!(asked.len() <= MAX_QUEUED);
}

#[test]
fn it_says_where_it_is() {
    let Some(mut g) = load() else { return };
    opened(&mut g);
    let r = call(&mut g, |g| Instance::call(g, &status_action(), &[]));
    let Val::String(said) = r.unwrap().unwrap() else {
        panic!("not a string");
    };
    assert!(
        said.contains("src/lib.rs") && said.contains("line 2"),
        "{said}"
    );
}

#[test]
fn a_new_instance_comes_back_to_an_empty_editor_from_a_checkpoint() {
    let Some(wasm) = wasm() else { return };
    let mut g = start(&wasm);
    frame(&mut g);
    let saved = Instance::export(&mut g, 0).unwrap().unwrap();
    let mut h = start(&wasm);
    assert_eq!(h.import(&saved, 1), Ok(0));
    assert_eq!(frame(&mut h).1.title, "Editor");
    // A state it cannot read and another schema are refused, not guessed at.
    assert_eq!(h.import(b"junk", 1), Ok(2));
    assert_eq!(h.import(&saved, 9), Ok(1));
}

/// The window's last frame, whatever else was sent after it.
fn last_frame(g: &mut CoreInstance) -> uikit::Frame {
    g.take_ui()
        .into_iter()
        .rev()
        .find_map(|u| match u {
            Update::Frame { frame, .. } => Some(frame),
            _ => None,
        })
        .expect("a frame")
}

#[test]
fn a_window_with_text_not_kept_says_so_and_asks_before_it_closes() {
    let Some(wasm) = wasm() else { return };
    let mut g = start(&wasm);
    let w = opened(&mut g);
    // Nothing typed: the frame does not say unsaved, and a close is a close.
    call(&mut g, |g| g.ui_events(w, &[key("down", "", 0)])).unwrap();
    chain(&mut g);
    assert!(!last_frame(&mut g).unsaved);

    // Text typed into a source that is not built: it says so, to the host.
    call(&mut g, |g| g.ui_events(w, &[key("x", "x", 0)])).unwrap();
    chain(&mut g);
    let f = last_frame(&mut g);
    assert!(f.unsaved, "{}", text_of(&f));

    // The close asks, with Discard and Cancel (a source has nothing to save).
    call(&mut g, |g| g.ui_events(w, &[UiEvent::CloseRequested])).unwrap();
    let updates = g.take_ui();
    assert!(
        !updates.iter().any(|u| matches!(u, Update::Closed(_))),
        "the window closed with text not kept"
    );
    let asked = updates
        .into_iter()
        .rev()
        .find_map(|u| match u {
            Update::Frame { frame, .. } => Some(frame),
            _ => None,
        })
        .expect("a frame");
    let text = text_of(&asked);
    assert!(
        text.contains("Discard") && text.contains("Cancel"),
        "{text}"
    );
    assert!(!text.contains("Save"), "{text}");
    assert!(asked.unsaved);

    // Cancel goes back to editing; Discard closes.
    call(&mut g, |g| g.ui_events(w, &[UiEvent::Pressed(CANCEL)])).unwrap();
    let back = last_frame(&mut g);
    assert!(!text_of(&back).contains("Discard"), "{}", text_of(&back));
    call(&mut g, |g| g.ui_events(w, &[UiEvent::CloseRequested])).unwrap();
    g.take_ui();
    call(&mut g, |g| g.ui_events(w, &[UiEvent::Pressed(SAVE)])).unwrap();
    assert!(
        !g.take_ui().iter().any(|u| matches!(u, Update::Closed(_))),
        "there is nothing to save, so Save does nothing"
    );
    call(&mut g, |g| g.ui_events(w, &[UiEvent::Pressed(DISCARD)])).unwrap();
    assert!(
        g.take_ui()
            .iter()
            .any(|u| matches!(u, Update::Closed(id) if *id == w)),
        "Discard closes the window"
    );
}

/// The agent opens `path` and the door answers with `text`: the editor has it
/// open, in a window with the id given.
fn open_the_file(g: &mut CoreInstance, path: &str, text: &str) -> u32 {
    let (w, _) = frame(g);
    let open_file = Func {
        export: String::from("app:editor/actions@1.0.0#open-file"),
        params: vec![Ty::String],
        result: Some(Ty::Result(
            Some(Box::new(Ty::String)),
            Some(Box::new(Ty::String)),
        )),
    };
    call(g, |g| {
        Instance::call(g, &open_file, &[Val::String(path.into())])
    })
    .unwrap()
    .unwrap();
    let asked = g.take_files();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(matches!(&asked[0].op, FileOp::Read { path: p } if p == path));
    call(g, |g| {
        g.files_reply(asked[0].ticket, Ok(text.as_bytes().to_vec()))
    })
    .unwrap();
    g.take_ui();
    w
}

/// A new instance from `saved`, as a nap's wake makes one: it imports (a
/// migration, which takes no file request), and its first call reads `path`,
/// which the door answers with `disk`. The window's last frame comes back.
fn wake(wasm: &[u8], limits: Limits, saved: &[u8], path: &str, disk: &str) -> uikit::Frame {
    let mut h = start_with(wasm, limits);
    assert_eq!(h.import(saved, 1), Ok(0));
    assert!(h.take_files().is_empty());
    let (w, _) = frame(&mut h);
    call(&mut h, |h| h.code_size(w, CODE, 10, 60)).unwrap();
    let again = h.take_files();
    assert_eq!(again.len(), 1, "{again:?}");
    assert!(matches!(&again[0].op, FileOp::Read { path: p } if p == path));
    call(&mut h, |h| {
        h.files_reply(again[0].ticket, Ok(disk.as_bytes().to_vec()))
    })
    .unwrap();
    last_frame(&mut h)
}

/// A checkpoint of an editor with `PATH` open and `x` typed into its first line,
/// `milk` being the disk's text.
const KEPT_PATH: &str = "/home/user/todo.txt";

fn kept_checkpoint(wasm: &[u8]) -> Vec<u8> {
    let mut g = start(wasm);
    let w = open_the_file(&mut g, KEPT_PATH, "buy milk\ncall Ann\n");
    call(&mut g, |g| g.ui_events_user(w, &[key("x", "x", 0)])).unwrap();
    Instance::export(&mut g, 0).unwrap().unwrap()
}

/// What a checkpoint of `path` with `text` kept looks like, built by hand.
fn checkpoint_of(path: &str, text: &str) -> Vec<u8> {
    let kept = comp_editor_window::V1 {
        path: Some(path.into()),
        text: Some(text.into()),
        line: Some(1),
        ..comp_editor_window::V1::default()
    };
    compsdk::state::encode(comp_editor_window::SCHEMA, &kept)
}

/// A new instance from `saved` that has imported and not yet had a call.
fn imported(wasm: &[u8], saved: &[u8]) -> CoreInstance {
    let mut h = start(wasm);
    assert_eq!(h.import(saved, 1), Ok(0));
    assert!(h.take_files().is_empty());
    h
}

/// Gives `h` its first transaction, which asks for the file, and answers that
/// with `outcome`. The window's id and last frame.
fn first_call_answered(
    h: &mut CoreInstance,
    outcome: Result<Vec<u8>, String>,
) -> (u32, uikit::Frame) {
    let (w, _) = frame(h);
    call(h, |h| h.code_size(w, CODE, 10, 60)).unwrap();
    let asked = h.take_files();
    assert_eq!(asked.len(), 1, "{asked:?}");
    call(h, |h| h.files_reply(asked[0].ticket, outcome)).unwrap();
    (w, last_frame(h))
}

/// Ctrl-S as a person's key in window `w`: the write it asks the door for, as
/// `(path, data, create)`.
fn saved_by_ctrl_s(h: &mut CoreInstance, w: u32) -> (String, Vec<u8>, bool) {
    call(h, |h| h.ui_events_user(w, &[key("s", "", CTRL)])).unwrap();
    let mut asked = h.take_files();
    assert_eq!(asked.len(), 1, "{asked:?}");
    match asked.remove(0).op {
        FileOp::Write { path, data, create } => (path, data, create),
        other => panic!("{other:?}"),
    }
}

fn open_file_action(g: &mut CoreInstance, path: &str) -> Result<String, String> {
    let func = Func {
        export: String::from("app:editor/actions@1.0.0#open-file"),
        params: vec![Ty::String],
        result: Some(Ty::Result(
            Some(Box::new(Ty::String)),
            Some(Box::new(Ty::String)),
        )),
    };
    let r = call(g, |g| Instance::call(g, &func, &[Val::String(path.into())]));
    match r.unwrap().unwrap() {
        Val::Result(Ok(Some(v))) => match *v {
            Val::String(s) => Ok(s),
            other => panic!("{other:?}"),
        },
        Val::Result(Err(Some(v))) => match *v {
            Val::String(s) => Err(s),
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_open_by_the_agent_right_after_a_wake_does_not_drop_the_text_kept() {
    let Some(wasm) = wasm() else { return };
    let saved = kept_checkpoint(&wasm);
    let mut h = imported(&wasm, &saved);
    // The first call after the wake is the agent's: it is refused, and asks the
    // door for nothing.
    let why = open_file_action(&mut h, "/home/user/other.txt").unwrap_err();
    assert!(why.contains("not saved"), "{why}");
    assert!(open(&mut h, "notes", "src/lib.rs", 1).is_err());
    // The refused call asked for the kept file, so the restore goes on without
    // waiting for another call: one read, and not one for the open.
    let asked = h.take_files();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(matches!(&asked[0].op, FileOp::Read { path } if path == KEPT_PATH));
    call(&mut h, |h| {
        h.files_reply(asked[0].ticket, Ok(b"buy milk\ncall Ann\n".to_vec()))
    })
    .unwrap();
    let back = last_frame(&mut h);
    let text = text_of(&back);
    assert!(text.contains("xbuy milk") && back.unsaved, "{text}");
    // While it is unsaved the editor still refuses an open that would drop it.
    assert!(open_file_action(&mut h, "/home/user/other.txt").is_err());
    assert!(h.take_files().is_empty());
}

#[test]
fn an_open_while_the_kept_files_read_is_out_is_refused_too() {
    let Some(wasm) = wasm() else { return };
    let saved = kept_checkpoint(&wasm);
    let mut h = imported(&wasm, &saved);
    let (w, _) = frame(&mut h);
    call(&mut h, |h| h.code_size(w, CODE, 10, 60)).unwrap();
    let asked = h.take_files();
    assert_eq!(asked.len(), 1, "{asked:?}");
    // The read is out: an open is refused, and the answer then still has its text.
    assert!(open_file_action(&mut h, "/home/user/other.txt").is_err());
    assert!(open(&mut h, "notes", "src/lib.rs", 1).is_err());
    assert!(h.take_files().is_empty());
    call(&mut h, |h| {
        h.files_reply(asked[0].ticket, Ok(b"buy milk\ncall Ann\n".to_vec()))
    })
    .unwrap();
    assert!(text_of(&last_frame(&mut h)).contains("xbuy milk"));
}

#[test]
fn text_kept_survives_a_file_that_cannot_be_opened_again() {
    let Some(wasm) = wasm() else { return };
    let saved = kept_checkpoint(&wasm);
    // The read is refused.
    let mut h = imported(&wasm, &saved);
    let there = format!("{KEPT_PATH}.recovered-0");
    let (w, back) = first_call_answered(&mut h, Err(String::from("denied: not yours")));
    let text = text_of(&back);
    assert!(text.contains("xbuy milk") && back.unsaved, "{text}");
    assert!(text.contains(&there), "{text}");
    // A save makes the new name, never the file that could not be read.
    assert_eq!(
        saved_by_ctrl_s(&mut h, w),
        (there.clone(), b"xbuy milk\ncall Ann\n".to_vec(), true)
    );
    // The file is not text any more.
    let mut h = imported(&wasm, &saved);
    let (w, back) = first_call_answered(&mut h, Ok(vec![0xff, 0xfe, 0x00]));
    let text = text_of(&back);
    assert!(text.contains("xbuy milk") && back.unsaved, "{text}");
    assert!(text.contains("not text") && text.contains(&there), "{text}");
    assert_eq!(saved_by_ctrl_s(&mut h, w).0, there);
    // The file grew past what the editor opens.
    let mut h = imported(&wasm, &saved);
    let big = vec![b'a'; editcore::file::MAX_BYTES + 1];
    let (w, back) = first_call_answered(&mut h, Ok(big));
    let text = text_of(&back);
    assert!(text.contains("xbuy milk") && back.unsaved, "{text}");
    assert_eq!(saved_by_ctrl_s(&mut h, w).0, there);
}

#[test]
fn a_recovered_file_that_naps_again_comes_back_at_its_own_name() {
    let Some(wasm) = wasm() else { return };
    let saved = kept_checkpoint(&wasm);
    let mut h = imported(&wasm, &saved);
    first_call_answered(&mut h, Err(String::from("denied: not yours")));
    // The checkpoint of the recovered editor names the new file, with the text.
    let there = format!("{KEPT_PATH}.recovered-0");
    let again = Instance::export(&mut h, 0).unwrap().unwrap();
    let kept: comp_editor_window::V1 =
        compsdk::state::decode(&again, comp_editor_window::SCHEMA).unwrap();
    assert_eq!(kept.path.as_deref(), Some(there.as_str()));
    assert_eq!(kept.text.as_deref(), Some("xbuy milk\ncall Ann\n"));
    // The new file is not on the disk yet, so it opens as a new file with the
    // text, and a save makes it.
    let mut h = imported(&wasm, &again);
    let (w, back) = first_call_answered(&mut h, Err(String::from("missing: no such file")));
    let text = text_of(&back);
    assert!(text.contains("xbuy milk") && back.unsaved, "{text}");
    assert_eq!(
        saved_by_ctrl_s(&mut h, w),
        (there, b"xbuy milk\ncall Ann\n".to_vec(), true)
    );
}

#[test]
fn text_kept_survives_a_path_the_door_would_not_be_asked_for() {
    let Some(wasm) = wasm() else { return };
    // A checkpoint whose path the editor no longer asks the door for: no read,
    // and the text is shown as a new file at the path, unsaved, with the reason.
    let saved = checkpoint_of("/etc/hosts", "kept text\n");
    let mut h = imported(&wasm, &saved);
    let (w, _) = frame(&mut h);
    call(&mut h, |h| h.code_size(w, CODE, 10, 60)).unwrap();
    assert!(h.take_files().is_empty());
    let back = last_frame(&mut h);
    let text = text_of(&back);
    assert!(text.contains("kept text") && back.unsaved, "{text}");
    assert!(text.contains("could not be opened again"), "{text}");
    // It is saved in the person's folder, as a new file, not at that path.
    assert_eq!(
        saved_by_ctrl_s(&mut h, w),
        (
            String::from("/home/user/recovered-0.txt"),
            b"kept text\n".to_vec(),
            true
        )
    );
}

#[test]
fn a_checkpoint_taken_while_the_kept_files_read_is_out_keeps_the_path_and_the_text() {
    let Some(wasm) = wasm() else { return };
    let saved = kept_checkpoint(&wasm);
    let mut h = imported(&wasm, &saved);
    let (w, _) = frame(&mut h);
    call(&mut h, |h| h.code_size(w, CODE, 10, 60)).unwrap();
    assert_eq!(h.take_files().len(), 1);
    // The editor naps again before the door answers.
    let again = Instance::export(&mut h, 0).unwrap().unwrap();
    let kept: comp_editor_window::V1 =
        compsdk::state::decode(&again, comp_editor_window::SCHEMA).unwrap();
    assert_eq!(kept.path.as_deref(), Some(KEPT_PATH));
    assert_eq!(kept.text.as_deref(), Some("xbuy milk\ncall Ann\n"));
    assert_eq!(kept.line, Some(1));
    // And what it kept brings the text back.
    let back = wake(&wasm, LIMITS, &again, KEPT_PATH, "buy milk\ncall Ann\n");
    let text = text_of(&back);
    assert!(text.contains("xbuy milk") && back.unsaved, "{text}");
}

#[test]
fn a_checkpoint_with_text_not_saved_brings_it_back_unsaved() {
    const PATH: &str = "/home/user/todo.txt";
    let Some(wasm) = wasm() else { return };
    let mut g = start(&wasm);
    let w = open_the_file(&mut g, PATH, "buy milk\ncall Ann\n");
    // A key typed into the file: the line is still being typed.
    call(&mut g, |g| g.ui_events_user(w, &[key("x", "x", 0)])).unwrap();
    let typed = last_frame(&mut g);
    assert!(typed.unsaved, "{}", text_of(&typed));
    let saved = Instance::export(&mut g, 0).unwrap().unwrap();
    let kept: comp_editor_window::V1 =
        compsdk::state::decode(&saved, comp_editor_window::SCHEMA).unwrap();
    assert_eq!(kept.path.as_deref(), Some(PATH));
    assert_eq!(kept.text.as_deref(), Some("xbuy milk\ncall Ann\n"));
    assert_eq!(kept.line, Some(1));

    // The disk has the file as it was: what comes back is the kept text, and
    // the window says so.
    let back = wake(&wasm, LIMITS, &saved, PATH, "buy milk\ncall Ann\n");
    let text = text_of(&back);
    assert!(text.contains("xbuy milk"), "{text}");
    assert!(back.unsaved, "{text}");
}

#[test]
fn a_file_near_the_limit_with_text_not_saved_is_kept_whole_in_a_checkpoint_that_fits() {
    const PATH: &str = "/home/user/big.txt";
    let Some(wasm) = wasm() else { return };
    // 1,900 lines of 31 bytes: 58,900 bytes, under the 60 KiB the editor takes.
    let disk: String = (0..1900)
        .map(|i| format!("line {i:04} {}\n", "-".repeat(20)))
        .collect();
    assert!(disk.len() > 58_000 && disk.len() < editcore::MAX_BUNDLE);
    let mut g = start_with(&wasm, BIG);
    let w = open_the_file(&mut g, PATH, &disk);
    call(&mut g, |g| g.ui_events_user(w, &[key("x", "x", 0)])).unwrap();
    let saved = Instance::export(&mut g, 0).unwrap().unwrap();
    // The checkpoint holds the whole text and is inside what the host takes.
    assert!(saved.len() > 58_000, "{}", saved.len());
    assert!(saved.len() <= compsdk::MAX_STATE, "{}", saved.len());
    let kept: comp_editor_window::V1 =
        compsdk::state::decode(&saved, comp_editor_window::SCHEMA).unwrap();
    assert_eq!(kept.text.map(|t| t.len()), Some(disk.len() + 1));

    let back = wake(&wasm, BIG, &saved, PATH, &disk);
    let text = text_of(&back);
    assert!(text.contains("xline 0000"), "{text}");
    assert!(back.unsaved, "{text}");
}

#[test]
fn a_file_with_nothing_unsaved_keeps_only_its_path() {
    const PATH: &str = "/home/user/todo.txt";
    let Some(wasm) = wasm() else { return };
    let mut g = start(&wasm);
    open_the_file(&mut g, PATH, "buy milk\n");
    let saved = Instance::export(&mut g, 0).unwrap().unwrap();
    let kept: comp_editor_window::V1 =
        compsdk::state::decode(&saved, comp_editor_window::SCHEMA).unwrap();
    assert_eq!((kept.path.as_deref(), kept.text), (Some(PATH), None));
    let back = wake(&wasm, LIMITS, &saved, PATH, "buy milk\n");
    assert!(text_of(&back).contains("buy milk"));
    assert!(!back.unsaved);
}

#[test]
fn a_checkpoint_with_a_file_open_imports_and_reads_the_file_in_the_first_call_after() {
    const PATH: &str = "/home/user/todo.txt";
    let Some(wasm) = wasm() else { return };
    let mut g = start(&wasm);
    frame(&mut g);
    // The agent opens a file and the door answers: the editor has it open.
    let open_file = Func {
        export: String::from("app:editor/actions@1.0.0#open-file"),
        params: vec![Ty::String],
        result: Some(Ty::Result(
            Some(Box::new(Ty::String)),
            Some(Box::new(Ty::String)),
        )),
    };
    call(&mut g, |g| {
        Instance::call(g, &open_file, &[Val::String(PATH.into())])
    })
    .unwrap()
    .unwrap();
    let asked = g.take_files();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(matches!(&asked[0].op, FileOp::Read { path } if path == PATH));
    call(&mut g, |g| {
        g.files_reply(
            asked[0].ticket,
            Ok(b"buy milk
call Ann
"
            .to_vec()),
        )
    })
    .unwrap();
    let saved = Instance::export(&mut g, 0).unwrap().unwrap();

    // A new instance imports it, as a nap's wake does. The import is a
    // migration, which the host takes no file request from, so it must ask
    // for none: it imports (it would not if it did) and nothing is queued.
    let mut h = start(&wasm);
    assert_eq!(h.import(&saved, 1), Ok(0));
    assert!(h.take_files().is_empty());
    let (w, _) = frame(&mut h);

    // The first call that is a transaction reads the file again, and the
    // answer fills the window.
    call(&mut h, |h| h.code_size(w, CODE, 10, 60)).unwrap();
    let again = h.take_files();
    assert_eq!(again.len(), 1, "{again:?}");
    assert!(matches!(&again[0].op, FileOp::Read { path } if path == PATH));
    call(&mut h, |h| {
        h.files_reply(
            again[0].ticket,
            Ok(b"buy milk
call Ann
"
            .to_vec()),
        )
    })
    .unwrap();
    let text = text_of(&last_frame(&mut h));
    assert!(text.contains("buy milk"), "{text}");
    // And only once.
    call(&mut h, |h| h.code_size(w, CODE, 12, 60)).unwrap();
    assert!(h.take_files().is_empty());
}
