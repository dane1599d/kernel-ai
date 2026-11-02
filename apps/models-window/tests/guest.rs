//! The model manager component as built for wasm32, run in `wasmhost` with no
//! OS: it opens its window when it starts and asks for the list, draws each list
//! the host gives, shows a choice as what to ask Kai with no control bound to a
//! row and no effect queued, and asks again on Refresh. It is skipped when
//! `cargo build -p comp-models-window --target wasm32-unknown-unknown --profile
//! component` has not run (CI runs it first).

use comp_models::{Models, Sha256, Signed, Store, Stored};
use comp_models_ui::{REFRESH, TABLE, USE};
use livecore::txn::{Effect, Inputs, Storage, Txn};
use livecore::{Instance, Reason};
use uikit::a11y::Action;
use uikit::widget::Value;
use wasmhost::{CoreInstance, Grants, Limits, Update, load_core, load_core_granted};

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

const FIXTURE: Sha256 = [0xcc; 32];
const FIXTURE_2: Sha256 = [0xdd; 32];

fn path() -> String {
    format!(
        "{}/../../target/wasm32-unknown-unknown/component/comp_models_window.wasm",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn granted() -> Grants {
    Grants {
        models: true,
        ..Grants::default()
    }
}

fn wasm() -> Option<Vec<u8>> {
    let wasm = std::fs::read(path()).ok();
    if wasm.is_none() {
        eprintln!("skipped: build comp-models-window for wasm32 first");
    }
    wasm
}

fn load() -> Option<CoreInstance> {
    let mut g = load_core_granted(&wasm()?, LIMITS, None, granted()).unwrap();
    // As the host loads it: storage readable, empty.
    g.view(&Storage::new());
    assert_eq!(g.init(), Ok(0));
    Some(g)
}

/// Runs `f` as the host runs a call, in a transaction.
fn call<R>(g: &mut CoreInstance, f: impl FnOnce(&mut CoreInstance) -> R) -> (R, Txn) {
    g.begin(&Storage::new(), Inputs::default());
    let out = f(g);
    (out, g.finish())
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

fn signed(id: &str, name: &str, sha256: Sha256) -> Signed {
    Signed {
        id: id.into(),
        name: name.into(),
        length: 2_101_245,
        sha256,
        licence: "MIT".into(),
        source: String::new(),
    }
}

/// `updated`'s list: fixture runs, and fixture-2 is stored.
fn list() -> String {
    let models = Models {
        signed: vec![
            signed("fixture", "Fixture model", FIXTURE),
            signed("fixture-2", "Second fixture model", FIXTURE_2),
        ],
        store: Store {
            blobs: [FIXTURE, FIXTURE_2]
                .map(|sha256| Stored {
                    sha256,
                    length: 2_101_245,
                })
                .to_vec(),
            running: Some(FIXTURE),
            ..Store::default()
        },
        ..Models::default()
    };
    modelchan::listing::encode(&models)
}

/// The host answers the read with this ticket.
fn listing(g: &mut CoreInstance, ticket: u32, outcome: Result<String, String>) -> Vec<String> {
    let (r, txn) = call(g, |g| g.models_listing(ticket, outcome));
    r.unwrap();
    effects(&txn)
}

fn press(g: &mut CoreInstance, w: u32, node: u32) -> Vec<String> {
    let (r, txn) = call(g, |g| g.ui_act(w, node, Action::Click, None));
    assert_eq!(r, Ok(Ok(())));
    effects(&txn)
}

#[test]
fn the_window_reads_the_list_and_sends_a_switch_to_kai() {
    let Some(mut g) = load() else { return };

    // Its window is there at once, with no models in it, and one read waits.
    let (w, f) = frame(&mut g);
    assert_eq!(f.title, "Models");
    assert!(!names(&f).iter().any(|n| n.contains("fixture")));
    assert_eq!(g.take_models(), [0]);

    // The list fills the table, with each model's licence and state, and the
    // window asks for nothing more until Refresh.
    assert!(listing(&mut g, 0, Ok(list())).is_empty());
    let (_, f) = frame(&mut g);
    let table = f.a11y.iter().find(|n| n.id == TABLE).unwrap();
    let rows = table.value.as_deref().unwrap();
    assert!(rows.starts_with("2 rows"), "{rows}");
    assert!(!g.models_waiting());

    // Choosing the stored model and its button sends the user to Kai: no
    // effect, and no control in the window is bound to a row.
    let (r, txn) = call(&mut g, |g| {
        g.ui_act(w, TABLE, Action::SetValue, Some(Value::Index(1)))
    });
    assert_eq!(r, Ok(Ok(())));
    assert!(effects(&txn).is_empty());
    frame(&mut g);
    assert!(press(&mut g, w, USE).is_empty());
    let (_, f) = frame(&mut g);
    assert!(
        names(&f)
            .iter()
            .any(|n| n.contains("ask Kai: \"use the fixture-2 model\"")),
        "{:?}",
        names(&f)
    );
    assert!(f.a11y.iter().all(|n| n.bound.is_none()));

    // Refresh asks for the list again.
    assert!(press(&mut g, w, REFRESH).is_empty());
    assert_eq!(g.take_models(), [1]);
}

#[test]
fn a_list_that_could_not_be_read_says_why() {
    let Some(mut g) = load() else { return };
    frame(&mut g);
    assert_eq!(g.take_models(), [0]);
    listing(
        &mut g,
        0,
        Err("there is no model store: no data disk".into()),
    );
    let (_, f) = frame(&mut g);
    assert!(
        names(&f)
            .iter()
            .any(|n| n == "The list could not be read: there is no model store: no data disk"),
        "{:?}",
        names(&f)
    );
    // Text that is not `kai-models 1` is not shown as a list.
    listing(&mut g, 0, Ok("models: none\n".into()));
    let (_, f) = frame(&mut g);
    assert!(
        names(&f)
            .iter()
            .any(|n| n == "The list came in a form this version does not know."),
        "{:?}",
        names(&f)
    );
}

#[test]
fn without_the_models_grant_the_window_does_not_load() {
    let Some(wasm) = wasm() else { return };
    let fault = match load_core(&wasm, LIMITS, None) {
        Err(fault) => fault,
        Ok(_) => panic!("loaded without the grant"),
    };
    assert_eq!(fault.reason, Reason::Link);
    assert!(fault.detail.contains("models grant"), "{}", fault.detail);
}
