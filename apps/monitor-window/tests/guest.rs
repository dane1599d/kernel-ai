//! The Monitor component as built for wasm32, run in `wasmhost` with no OS: it
//! opens its window when it starts and asks for a sample, turns each sample the
//! host gives into the window and asks for the next, queues `process_stop` as an
//! effect when a click or Stop asks for it, and stops reading when its window is
//! closed. It is skipped when `cargo build -p comp-monitor-window --target
//! wasm32-unknown-unknown --profile component` has not run (CI runs it first).

use appabi::{Func, Ty, Val};
use comp_monitor_ui::{SORT_NAME, STOP, TABLE};
use livecore::txn::{Effect, Inputs, Storage, Txn};
use livecore::{Instance, Reason};
use systemabi::{ModelState, ModelStatus, ProcSample, Sample};
use uikit::a11y::Action;
use uikit::widget::{UiEvent, Value};
use wasmhost::{AppEvent, CoreInstance, Grants, Limits, Update, load_core, load_core_granted};

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

const MS: u64 = 1_000_000;
const SEC: u64 = 1_000_000_000;

fn path() -> String {
    format!(
        "{}/../../target/wasm32-unknown-unknown/component/comp_monitor_window.wasm",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn granted() -> Grants {
    Grants {
        system: true,
        ..Grants::default()
    }
}

fn load() -> Option<CoreInstance> {
    let Ok(wasm) = std::fs::read(path()) else {
        eprintln!("skipped: build comp-monitor-window for wasm32 first");
        return None;
    };
    NEXT.set(1);
    let mut g = load_core_granted(&wasm, LIMITS, None, granted()).unwrap();
    // As the host loads it: storage readable, empty.
    g.view(&Storage::new());
    assert_eq!(g.init(), Ok(0));
    Some(g)
}

thread_local! {
    /// The ticket the next effect gets: the host numbers them across calls.
    static NEXT: std::cell::Cell<u32> = const { std::cell::Cell::new(1) };
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

fn shown(f: &uikit::Frame) -> String {
    f.list.to_string()
}

fn proc(pid: u32, name: &str, pages: u64, cpu_ns: u64) -> ProcSample {
    ProcSample {
        pid,
        name: name.into(),
        pages,
        cpu_ns,
    }
}

/// A 4-CPU machine `seconds` after a minute of uptime; llm uses 61.2% of a CPU
/// and desktop 8%.
fn usual(seconds: u64) -> Vec<u8> {
    Sample {
        uptime_ns: 60 * SEC + seconds * SEC,
        cpus: 4,
        free_pages: 1500 * 256,
        total_pages: 4096 * 256,
        heap_bytes: 820 * 1024,
        procs: vec![
            proc(1, "supervisor", 75, 5 * MS * seconds),
            proc(4, "llm", 600_000, 612 * MS * seconds),
            proc(7, "desktop", 10_250, 80 * MS * seconds),
            proc(9, "shell", 40, 0),
        ],
        omitted: 0,
        model: ModelStatus {
            state: ModelState::Ready,
            tps10: 125,
        },
    }
    .encode()
}

/// The host answers the read with this ticket.
fn sample(g: &mut CoreInstance, ticket: u32, outcome: Result<Vec<u8>, String>) -> Vec<String> {
    let (r, txn) = call(g, |g| g.system_sample(ticket, outcome));
    r.unwrap();
    effects(&txn)
}

fn done(g: &mut CoreInstance, ticket: u32, outcome: Result<&str, &str>) {
    let outcome = outcome.map(String::from).map_err(String::from);
    let (r, _) = call(g, |g| {
        g.deliver_event(&AppEvent::EffectDone { ticket, outcome })
    });
    r.unwrap();
}

fn action(name: &str) -> Func {
    Func {
        export: format!("app:monitor/actions@1.0.0#{name}"),
        params: vec![Ty::String],
        result: Some(Ty::Result(
            Some(Box::new(Ty::String)),
            Some(Box::new(Ty::String)),
        )),
    }
}

#[test]
fn the_monitor_guest_reads_the_machine_and_asks_the_os_to_stop_a_process() {
    let Some(mut g) = load() else { return };

    // Its window is there at once, with nothing in it, and one read waits.
    let (w, f) = frame(&mut g);
    assert_eq!(f.title, "System Monitor");
    assert!(!shown(&f).contains("llm"));
    assert!(g.system_waiting());
    assert_eq!(g.take_system(), [0]);
    assert!(!g.system_waiting());

    // The first sample fills the table (no shares yet) and asks for the next.
    assert_eq!(sample(&mut g, 0, Ok(usual(1))), [""; 0]);
    let (_, f) = frame(&mut g);
    assert!(shown(&f).contains("llm") && shown(&f).contains("supervisor"));
    assert_eq!(g.take_system(), [1]);

    // The second gives the shares and the model's line.
    sample(&mut g, 1, Ok(usual(2)));
    let (_, f) = frame(&mut g);
    assert!(shown(&f).contains("61.2%"), "{}", shown(&f));
    assert!(
        names(&f).iter().any(|n| n.starts_with("Model: ready")),
        "{:?}",
        names(&f)
    );
    assert_eq!(g.take_system(), [2]);

    // Picking the busiest row and pressing Stop asks the OS, by name.
    let (r, _) = call(&mut g, |g| {
        g.ui_act(w, TABLE, Action::SetValue, Some(Value::Index(0)))
    });
    assert_eq!(r, Ok(Ok(())));
    let (r, txn) = call(&mut g, |g| g.ui_events(w, &[UiEvent::Pressed(STOP)]));
    r.unwrap();
    assert_eq!(effects(&txn), [r#"process_stop {name: "llm"}"#]);
    g.take_ui();

    // The OS's refusal shows in the window.
    done(&mut g, 1, Err("the user said no"));
    let (_, f) = frame(&mut g);
    assert!(names(&f).contains(&String::from("the user said no")));
}

#[test]
fn a_reading_that_failed_is_said_and_the_next_good_sample_clears_it() {
    let Some(mut g) = load() else { return };
    frame(&mut g);
    assert_eq!(g.take_system(), [0]);
    sample(&mut g, 0, Ok(usual(1)));
    frame(&mut g);

    // Bytes that are not a sample, then the host's own error.
    sample(&mut g, 1, Ok(b"not a sample".to_vec()));
    let (_, f) = frame(&mut g);
    assert!(
        names(&f).iter().any(|n| n.contains("could not be read")),
        "{:?}",
        names(&f)
    );
    // The last picture stays, and the next read is out.
    assert!(shown(&f).contains("llm"));
    assert_eq!(g.take_system(), [1, 2]);
    sample(&mut g, 2, Err("the system could not be read".into()));
    let (_, f) = frame(&mut g);
    assert!(names(&f).iter().any(|n| n.contains("could not be read")));
    assert_eq!(g.take_system(), [3]);

    sample(&mut g, 3, Ok(usual(3)));
    let (_, f) = frame(&mut g);
    assert!(
        !names(&f).iter().any(|n| n.contains("could not be read")),
        "{:?}",
        names(&f)
    );
}

#[test]
fn closing_the_window_stops_the_reading() {
    let Some(mut g) = load() else { return };
    let (w, _) = frame(&mut g);
    assert_eq!(g.take_system(), [0]);
    let (r, _) = call(&mut g, |g| g.ui_events(w, &[UiEvent::CloseRequested]));
    r.unwrap();
    assert_eq!(g.take_ui(), vec![Update::Closed(w)]);

    // The answer that was on its way draws nothing and asks for nothing.
    sample(&mut g, 0, Ok(usual(1)));
    assert!(g.take_ui().is_empty());
    assert!(!g.system_waiting());
}

#[test]
fn the_agents_sort_and_select_reach_the_same_model() {
    let Some(mut g) = load() else { return };
    frame(&mut g);
    g.take_system();
    sample(&mut g, 0, Ok(usual(1)));
    sample(&mut g, 1, Ok(usual(2)));
    g.take_ui();

    let say = |g: &mut CoreInstance, name: &str, arg: &str| {
        call(g, |g| {
            Instance::call(g, &action(name), &[Val::String(arg.into())])
        })
        .0
        .unwrap()
        .unwrap()
    };
    let Val::Result(Ok(Some(said))) = say(&mut g, "sort", "name") else {
        panic!("sort should answer");
    };
    let Val::String(said) = *said else {
        panic!("a text");
    };
    assert!(said.starts_with("Sorted by name. First: desktop"), "{said}");
    let (_, f) = frame(&mut g);
    let list = shown(&f);
    assert!(list.find("desktop") < list.find("llm"));
    assert!(matches!(say(&mut g, "select", "llm"), Val::Result(Ok(_))));
    assert!(matches!(say(&mut g, "select", "nope"), Val::Result(Err(_))));
    assert!(matches!(say(&mut g, "sort", "colour"), Val::Result(Err(_))));
}

#[test]
fn it_comes_back_sorted_the_way_it_was_and_reads_again() {
    let Some(mut g) = load() else { return };
    let (w, _) = frame(&mut g);
    g.take_system();
    sample(&mut g, 0, Ok(usual(1)));
    sample(&mut g, 1, Ok(usual(2)));
    call(&mut g, |g| g.ui_events(w, &[UiEvent::Pressed(SORT_NAME)]))
        .0
        .unwrap();
    let saved = Instance::export(&mut g, 0).unwrap().unwrap();

    let Some(mut h) = load() else { return };
    assert_eq!(h.import(&saved, 1), Ok(0));
    frame(&mut h);
    // `load` ran `init` first, which asked for ticket 0; the import asked again.
    assert_eq!(h.take_system(), [0, 1]);
    sample(&mut h, 1, Ok(usual(1)));
    let (_, f) = frame(&mut h);
    let list = shown(&f);
    assert!(list.find("desktop") < list.find("llm"), "{list}");
    // A state it cannot read is refused, not guessed at.
    assert_eq!(h.import(b"junk", 1), Ok(2));
    assert_eq!(h.import(&saved, 9), Ok(1));
}

#[test]
fn an_app_without_the_system_grant_does_not_stage() {
    let Ok(wasm) = std::fs::read(path()) else {
        eprintln!("skipped: build comp-monitor-window for wasm32 first");
        return;
    };
    let fault = match load_core(&wasm, LIMITS, None) {
        Err(fault) => fault,
        Ok(_) => panic!("no grant, no staging"),
    };
    assert_eq!(fault.reason, Reason::Link);
    assert!(fault.detail.contains("system grant"), "{}", fault.detail);
}
