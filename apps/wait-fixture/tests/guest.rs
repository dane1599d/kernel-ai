//! The fixture as built for wasm32, run in `wasmhost` with no OS: Read queues
//! one `fs_list`, Make one `fs_create` of a new file, and what the OS says of
//! each is logged. Skipped when `cargo build -p comp-wait-fixture --target
//! wasm32-unknown-unknown --profile component` has not run (CI runs it first).

use comp_wait_fixture::{MAKE, READ};
use livecore::Instance;
use livecore::txn::{Effect, Inputs, Storage, Txn};
use std::cell::RefCell;
use uikit::widget::{NodeId, UiEvent};
use wasmhost::{AppEvent, CoreInstance, Limits, Update, load_core};

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

thread_local! {
    static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn log(line: &str) {
    LOG.with(|l| l.borrow_mut().push(line.to_string()));
}

fn lines() -> Vec<String> {
    LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

fn load() -> Option<CoreInstance> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/wasm32-unknown-unknown/component/comp_wait_fixture.wasm"
    );
    let Ok(wasm) = std::fs::read(path) else {
        eprintln!("skipped: build comp-wait-fixture for wasm32 first");
        return None;
    };
    let mut g = load_core(&wasm, LIMITS, Some(log)).unwrap();
    g.view(&Storage::new());
    assert_eq!(g.init(), Ok(0));
    Some(g)
}

fn call<R>(
    g: &mut CoreInstance,
    first_ticket: u32,
    now_ms: u64,
    f: impl FnOnce(&mut CoreInstance) -> R,
) -> (R, Txn) {
    let inputs = Inputs {
        first_ticket,
        now_ms,
        ..Inputs::default()
    };
    g.begin(&Storage::new(), inputs);
    let out = f(g);
    (out, g.finish())
}

fn press(
    g: &mut CoreInstance,
    window: u32,
    button: NodeId,
    ticket: u32,
    now_ms: u64,
) -> Vec<String> {
    let (r, txn) = call(g, ticket, now_ms, |g| {
        g.ui_events_user(window, &[UiEvent::Pressed(button)])
    });
    r.unwrap();
    txn.effects
        .iter()
        .map(|e| match e {
            Effect::Call { name, args } => format!("{name} {args}"),
            Effect::Notify(text) => format!("notify {text}"),
        })
        .collect()
}

fn done(g: &mut CoreInstance, ticket: u32, outcome: Result<&str, &str>, now_ms: u64) {
    let outcome = outcome.map(String::from).map_err(String::from);
    let (r, _) = call(g, ticket + 100, now_ms, |g| {
        g.deliver_event(&AppEvent::EffectDone { ticket, outcome })
    });
    r.unwrap();
}

#[test]
fn it_exports_its_own_state_and_takes_it_back() {
    let Some(mut g) = load() else { return };
    let saved = Instance::export(&mut g, 0).unwrap().unwrap();
    assert_eq!(livecore::state::schema_of(&saved), Some(1));
    let mut h = load().unwrap();
    assert_eq!(h.import(&saved, 1), Ok(0));
}

#[test]
fn read_and_make_queue_their_rows_and_each_answer_is_logged_with_its_time() {
    let Some(mut g) = load() else { return };
    let window = match g.take_ui().pop() {
        Some(Update::Frame { window, .. }) => window,
        other => panic!("{other:?}"),
    };
    lines();
    assert_eq!(
        press(&mut g, window, READ, 1, 1_000),
        ["fs_list {path: \"\"}"]
    );
    assert_eq!(
        press(&mut g, window, MAKE, 2, 1_100),
        ["fs_create {path: \"made-2.txt\", text: \"made by wait-window\"}"]
    );
    done(&mut g, 1, Ok("a\nb"), 1_250);
    done(&mut g, 2, Ok("Done: created made-2.txt"), 6_100);
    // An answer to a ticket it did not ask for says nothing.
    done(&mut g, 77, Ok("x"), 7_000);
    assert_eq!(
        lines(),
        [
            "wait-window: read ok in 250 ms",
            "wait-window: make ok in 5000 ms"
        ]
    );
}
