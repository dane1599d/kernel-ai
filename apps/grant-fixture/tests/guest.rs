//! The fixture as built for wasm32, run in `wasmhost` with no OS: pressing its
//! button queues one `fs_list`, and what the OS says of it is logged. Skipped
//! when `cargo build -p comp-grant-fixture --target wasm32-unknown-unknown
//! --profile component` has not run (CI runs it first).

use comp_grant_fixture::PING;
use livecore::Instance;
use livecore::txn::{Effect, Inputs, Storage, Txn};
use std::cell::RefCell;
use uikit::widget::UiEvent;
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
        "/../../target/wasm32-unknown-unknown/component/comp_grant_fixture.wasm"
    );
    let Ok(wasm) = std::fs::read(path) else {
        eprintln!("skipped: build comp-grant-fixture for wasm32 first");
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
    f: impl FnOnce(&mut CoreInstance) -> R,
) -> (R, Txn) {
    let inputs = Inputs {
        first_ticket,
        ..Inputs::default()
    };
    g.begin(&Storage::new(), inputs);
    let out = f(g);
    (out, g.finish())
}

fn press(g: &mut CoreInstance, window: u32, first_ticket: u32) -> Vec<String> {
    let (r, txn) = call(g, first_ticket, |g| {
        g.ui_events_user(window, &[UiEvent::Pressed(PING)])
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

fn done(g: &mut CoreInstance, ticket: u32, outcome: Result<&str, &str>) {
    let outcome = outcome.map(String::from).map_err(String::from);
    let (r, _) = call(g, ticket + 1, |g| {
        g.deliver_event(&AppEvent::EffectDone { ticket, outcome })
    });
    r.unwrap();
}

#[test]
fn a_press_queues_one_fs_list_and_the_outcome_is_logged() {
    let Some(mut g) = load() else { return };
    let window = match g.take_ui().pop() {
        Some(Update::Frame { window, .. }) => window,
        other => panic!("{other:?}"),
    };
    lines();
    // One press, one read-only row, the home folder.
    let queued = press(&mut g, window, 1);
    assert_eq!(queued, ["fs_list {path: \"\"}"]);
    done(&mut g, 1, Ok("a\nb"));
    assert_eq!(lines(), ["fixture: fs_list ok #1"]);
    // The OS refuses the next one: the app says so and keeps nothing.
    assert_eq!(press(&mut g, window, 2).len(), 1);
    done(&mut g, 2, Err("fs_list is withheld from fixture-window"));
    assert_eq!(
        lines(),
        ["fixture: fs_list refused #2: fs_list is withheld from fixture-window"]
    );
    // An answer to a ticket it did not ask for says nothing.
    done(&mut g, 77, Ok("x"));
    assert!(lines().is_empty());
}
