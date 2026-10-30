//! The Terminal component as built for wasm32, run in `wasmhost` with no OS: it
//! opens its window when it starts, turns a person's Enter and the AI's
//! `terminal_run` into one `shell_line` effect each inside a transaction, shows
//! the shell's answer when the OS delivers it, and comes back with its
//! scrollback. It is skipped when `cargo build -p comp-terminal-window --target
//! wasm32-unknown-unknown --profile component` has not run (CI runs it first).

use appabi::{Func, Ty, Val};
use comp_terminal_ui::{CANCEL, CLEAR, INPUT};
use livecore::Instance;
use livecore::txn::{Effect, Inputs, Storage, Txn};
use std::cell::Cell;
use uikit::widget::{UiEvent, Value};
use wasmhost::{AppEvent, CoreInstance, Limits, Update, load_core};

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

fn wasm() -> Option<Vec<u8>> {
    let target = std::env::var("CARGO_TARGET_DIR")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../target").to_string());
    let path = format!("{target}/wasm32-unknown-unknown/component/comp_terminal_window.wasm");
    let bytes = std::fs::read(path).ok();
    if bytes.is_none() {
        eprintln!("skipped: build comp-terminal-window for wasm32 first");
    }
    bytes
}

fn load() -> Option<CoreInstance> {
    let wasm = wasm()?;
    NEXT.set(1);
    let mut g = load_core(&wasm, LIMITS, None).unwrap();
    // As the host loads it: storage readable, empty.
    g.view(&Storage::new());
    assert_eq!(g.init(), Ok(0));
    Some(g)
}

thread_local! {
    /// The ticket the next effect gets: the host numbers them across calls.
    static NEXT: Cell<u32> = const { Cell::new(1) };
}

/// Runs `f` as the host runs a call: in a transaction, its effects numbered on
/// from the last call's.
fn call<R>(g: &mut CoreInstance, f: impl FnOnce(&mut CoreInstance) -> R) -> (R, Txn) {
    let first_ticket = NEXT.get();
    let inputs = Inputs {
        first_ticket,
        ..Inputs::default()
    };
    g.begin(&Storage::new(), inputs);
    let out = f(g);
    let txn = g.finish();
    NEXT.set(first_ticket + txn.effects.len() as u32);
    (out, txn)
}

fn effects(txn: &Txn) -> Vec<String> {
    txn.effects
        .iter()
        .map(|e| match e {
            Effect::Call { name, args } => format!("{name} {args}"),
            Effect::Notify(text) => format!("notify {text}"),
        })
        .collect()
}

/// The newest frame of the window the app has open.
fn frame(g: &mut CoreInstance) -> (u32, uikit::Frame) {
    match g.take_ui().pop() {
        Some(Update::Frame { window, frame }) => (window, frame),
        other => panic!("{other:?}"),
    }
}

fn names(f: &uikit::Frame) -> Vec<String> {
    f.a11y.iter().map(|n| n.name.clone()).collect()
}

fn done(g: &mut CoreInstance, ticket: u32, outcome: Result<&str, &str>) -> Vec<String> {
    let outcome = outcome.map(String::from).map_err(String::from);
    let (r, txn) = call(g, |g| {
        g.deliver_event(&AppEvent::EffectDone { ticket, outcome })
    });
    r.unwrap();
    effects(&txn)
}

fn action(name: &str, params: Vec<Ty>) -> Func {
    Func {
        export: format!("app:terminal/actions@1.0.0#{name}"),
        params,
        result: Some(Ty::Result(
            Some(Box::new(Ty::String)),
            Some(Box::new(Ty::String)),
        )),
    }
}

fn ok_text(r: Option<Val>) -> String {
    match r {
        Some(Val::Result(Ok(Some(text)))) => match *text {
            Val::String(text) => text,
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    }
}

/// A person types `line` and presses Enter.
fn type_line(g: &mut CoreInstance, w: u32, line: &str) -> Vec<String> {
    let (r, txn) = call(g, |g| {
        g.ui_events_user(
            w,
            &[
                UiEvent::Changed(INPUT, Value::Text(line.to_string())),
                UiEvent::Submitted(INPUT),
            ],
        )
    });
    r.unwrap();
    assert!(g.last_call_by_user(), "a key is a person's input");
    effects(&txn)
}

#[test]
fn a_typed_line_goes_to_the_shell_and_its_answer_comes_back() {
    let Some(mut g) = load() else { return };

    // The window opens at once, hello said, with the input line in focus.
    let (w, f) = frame(&mut g);
    assert_eq!(f.title, "Terminal");
    assert!(
        names(&f)
            .iter()
            .any(|n| n.starts_with("Kernel AI terminal."))
    );
    let input = f.a11y.iter().find(|n| n.id == INPUT).unwrap();
    assert_eq!(input.name, "kai>");
    assert!(input.focused);
    assert_eq!(f.a11y.iter().find(|n| n.id == CLEAR).unwrap().name, "Clear");
    call(&mut g, |g| g.deliver_event(&AppEvent::Started))
        .0
        .unwrap();

    // Enter queues the one effect; until the shell answers there is no input.
    assert_eq!(type_line(&mut g, w, "mem"), [r#"shell_line {line: "mem"}"#]);
    let (_, f) = frame(&mut g);
    assert!(names(&f).contains(&String::from("Running")));
    assert!(f.a11y.iter().all(|n| n.id != INPUT));

    // The answer is rows of the shell's kind, and the input is back in focus.
    assert_eq!(done(&mut g, 1, Ok("ok free 12 MiB\nheap 3 MiB")), [""; 0]);
    let (_, f) = frame(&mut g);
    let n = names(&f);
    assert!(n.contains(&String::from("= free 12 MiB")), "{n:?}");
    assert!(n.contains(&String::from("= heap 3 MiB")));
    let input = f.a11y.iter().find(|n| n.id == INPUT).unwrap();
    assert!(input.focused);
    assert_eq!(input.value.as_deref().unwrap_or(""), "");

    // An answer for no line of ours changes nothing.
    done(&mut g, 99, Ok("ok forged"));
    assert!(g.take_ui().is_empty());

    // An open entry: the prompt is `..> `, and Cancel drops it with one more effect.
    assert_eq!(
        type_line(&mut g, w, "if x {"),
        [r#"shell_line {line: "if x {"}"#]
    );
    done(&mut g, 2, Ok("more"));
    let (_, f) = frame(&mut g);
    assert_eq!(f.a11y.iter().find(|n| n.id == INPUT).unwrap().name, "..>");
    let (r, txn) = call(&mut g, |g| g.ui_events(w, &[UiEvent::Pressed(CANCEL)]));
    r.unwrap();
    assert_eq!(effects(&txn), [r#"shell_line {line: "\u{3}"}"#]);
    done(&mut g, 3, Ok("ok"));
    let (_, f) = frame(&mut g);
    assert_eq!(f.a11y.iter().find(|n| n.id == INPUT).unwrap().name, "kai>");

    // The OS refusing the effect shows as an error row, not as nothing.
    type_line(&mut g, w, "mem");
    done(&mut g, 4, Err("shell_line is not declared"));
    let (_, f) = frame(&mut g);
    assert!(names(&f).contains(&String::from(
        "! the OS did not run it: shell_line is not declared"
    )));

    // Close: the window goes.
    g.take_ui();
    let (r, _) = call(&mut g, |g| g.ui_events(w, &[UiEvent::CloseRequested]));
    r.unwrap();
    assert_eq!(g.take_ui(), vec![Update::Closed(w)]);
}

#[test]
fn the_ai_types_and_reads_and_a_person_runs_the_line() {
    let Some(mut g) = load() else { return };
    let (w, _) = frame(&mut g);

    // terminal_run only types: no effect is queued, and the line waits in the
    // input line for a person.
    let (r, txn) = call(&mut g, |g| {
        Instance::call(
            g,
            &action("run", vec![Ty::String]),
            &[Val::String("mem".into())],
        )
    });
    assert_eq!(ok_text(r.unwrap()), comp_terminal::TYPED);
    assert!(
        !g.last_call_by_user(),
        "an action call is not a person's input"
    );
    assert!(effects(&txn).is_empty());
    let (_, f) = frame(&mut g);
    let input = f.a11y.iter().find(|n| n.id == INPUT).unwrap();
    assert_eq!(input.value.as_deref(), Some("mem"));
    assert!(
        names(&f)
            .iter()
            .any(|n| n.starts_with("The AI typed a line for you"))
    );

    // It does not type over a field that has text in it.
    let (r, txn) = call(&mut g, |g| {
        Instance::call(
            g,
            &action("run", vec![Ty::String]),
            &[Val::String("ps".into())],
        )
    });
    assert!(matches!(r.unwrap(), Some(Val::Result(Err(_)))));
    assert!(effects(&txn).is_empty());

    // The person presses Enter on it as it is: one effect, and the row is the AI's.
    let (r, txn) = call(&mut g, |g| {
        g.ui_events_user(w, &[UiEvent::Submitted(INPUT)])
    });
    r.unwrap();
    assert!(g.last_call_by_user());
    assert_eq!(effects(&txn), [r#"shell_line {line: "mem"}"#]);
    let (_, f) = frame(&mut g);
    assert!(names(&f).contains(&String::from("by the AI")));

    // The shell's answer arrives; terminal_read gives the typed rows.
    done(&mut g, 1, Ok("ok free 12 MiB"));
    let (r, _) = call(&mut g, |g| {
        Instance::call(g, &action("read", vec![Ty::U32]), &[Val::U32(0)])
    });
    let read = ok_text(r.unwrap());
    let lines: Vec<&str> = read.lines().collect();
    assert!(
        lines[0].starts_with("next ") && lines[0].ends_with(" ready"),
        "{read}"
    );
    assert!(
        lines.iter().any(|l| l.ends_with(" input kai> mem")),
        "{read}"
    );
    assert!(
        lines.iter().any(|l| l.ends_with(" reply free 12 MiB")),
        "{read}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains(" notice The AI typed a line for you")),
        "{read}"
    );

    // terminal_clear empties the scrollback and the window shows it.
    let (r, _) = call(&mut g, |g| Instance::call(g, &action("clear", vec![]), &[]));
    assert_eq!(ok_text(r.unwrap()), "cleared");
    let (_, f) = frame(&mut g);
    assert!(!names(&f).iter().any(|n| n == "= free 12 MiB"));
}

#[test]
fn it_comes_back_with_its_scrollback_and_the_open_entry() {
    let Some(mut g) = load() else { return };
    let (w, _) = frame(&mut g);
    type_line(&mut g, w, "if x {");
    done(&mut g, 1, Ok("more"));
    // Its checkpoint holds the rows and that the entry is open; a new instance
    // imports them.
    let saved = Instance::export(&mut g, 1).unwrap().unwrap();
    let Some(mut h) = load() else { return };
    assert_eq!(h.import(&saved, 1), Ok(0));
    let (_, f) = frame(&mut h);
    let n = names(&f);
    assert!(n.iter().any(|t| t == "if x {"), "{n:?}");
    assert_eq!(f.a11y.iter().find(|n| n.id == INPUT).unwrap().name, "..>");
    // A state it cannot read is refused, not guessed at.
    assert_eq!(h.import(b"junk", 1), Ok(2));
    assert_eq!(h.import(&saved, 9), Ok(1));
}
