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

#[path = "guest/keys.rs"]
mod keys;

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
