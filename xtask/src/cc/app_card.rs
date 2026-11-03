//! The `c-app-1` card's host tests (plan "Kai builds apps", piece 2): the
//! template and every example `cc build` as window apps and run in wasmhost,
//! each call the card shows is `kaiapp.h`'s, and the card fits its budget.

use ccserve::build::{self, Build, Built, KAIAPP_H, Kind};
use ccserve::{Budget, Compiler, Host};
use livecore::Instance;
use livecore::txn::{Inputs, Storage};
use llmproto::pack::{self, capp1};
use std::sync::OnceLock;
use uikit::widget::UiEvent;
use wasmhost::{CoreInstance, Limits, LiveInstance, Update};

const LIMITS: Limits = Limits {
    fuel_call: 50_000_000,
    fuel_life: 200_000_000,
    memory_bytes: 4 << 20,
};

struct Clock;

impl Host for Clock {
    fn now_ns(&self) -> u64 {
        0
    }

    fn interrupted(&self) -> bool {
        false
    }
}

fn compiler() -> &'static Compiler {
    static COMPILER: OnceLock<Compiler> = OnceLock::new();
    COMPILER.get_or_init(|| {
        let cc = Compiler::new(ccserve::CC_WASM, &[]).expect("cc.wasm");
        build::compiler(&cc).expect("headers")
    })
}

/// `main` built as the app cc would build it, as the shell sends it.
fn built(main: &str) -> Built {
    let files = [("main.c", main.as_bytes())];
    assert_eq!(Kind::of(&files), Kind::Window);
    let mut budget = Budget::new(&Clock, u64::MAX / 4);
    let build = Build {
        name: "kaiapp",
        version: "0.0.1",
        kind: Kind::Window,
        files: &files,
    };
    build::build(compiler(), &build, &mut budget).unwrap_or_else(|e| panic!("{e:?}\n{main}"))
}

fn in_call<R>(app: &mut CoreInstance, f: impl FnOnce(&mut CoreInstance) -> R) -> R {
    let inputs = Inputs {
        now_ms: 0,
        seed: 0,
        first_ticket: 0,
    };
    app.begin(&Storage::new(), inputs);
    let result = f(app);
    app.finish();
    result
}

/// The app, started, and its window's id.
fn started(main: &str) -> (CoreInstance, u32) {
    let LiveInstance::V2(mut app) =
        wasmhost::load_live(&built(main).artifact, LIMITS, None).expect("it loads")
    else {
        panic!("a lifecycle 2 artifact");
    };
    assert_eq!(in_call(&mut app, |a| a.init()), Ok(0));
    let windows = app.ui_windows();
    assert_eq!(windows.len(), 1, "one window");
    (app, windows[0])
}

/// The names the last frame shows, in node order.
fn shown(app: &mut CoreInstance) -> Vec<String> {
    let frame = app
        .take_ui()
        .into_iter()
        .filter_map(|u| match u {
            Update::Frame { frame, .. } => Some(frame),
            _ => None,
        })
        .next_back()
        .expect("a frame");
    frame
        .a11y
        .iter()
        .map(|n| match &n.value {
            Some(value) => format!("{}={value}", n.name),
            None => n.name.clone(),
        })
        .collect()
}

fn names(app: &mut CoreInstance) -> String {
    shown(app).join("|")
}

#[test]
fn the_template_builds_opens_and_counts() {
    let (mut app, window) = started(capp1::TEMPLATE);
    let first = names(&mut app);
    assert!(first.contains("Count: 0"), "{first}");
    in_call(&mut app, |a| a.ui_events(window, &[UiEvent::Pressed(3)])).unwrap();
    in_call(&mut app, |a| a.ui_events(window, &[UiEvent::Pressed(3)])).unwrap();
    let after = names(&mut app);
    assert!(after.contains("Count: 2"), "{after}");
    in_call(&mut app, |a| a.ui_events(window, &[UiEvent::Pressed(4)])).unwrap();
    assert!(names(&mut app).contains("Count: 0"));
}

#[test]
fn every_example_builds_and_opens() {
    for (request, main, summary) in capp1::EXAMPLES {
        let (mut app, _) = started(main);
        assert!(!shown(&mut app).is_empty(), "{request}");
        assert!(summary.len() < llmproto::script::SUMMARY_MAX_CHARS);
        assert!(llmproto::script::check_text(main).is_ok(), "{request}");
    }
}

/// The shopping list keeps a field's text, adds it on Enter and clears.
#[test]
fn the_list_example_keeps_what_was_typed() {
    use uikit::widget::Value;
    let (_, main, _) = capp1::EXAMPLES[0];
    let (mut app, window) = started(main);
    let typed = UiEvent::Changed(1, Value::Text("milk".into()));
    in_call(&mut app, |a| {
        a.ui_events(window, &[typed, UiEvent::Submitted(1)])
    })
    .unwrap();
    let after = names(&mut app);
    assert!(after.contains("|milk"), "{after}");
    in_call(&mut app, |a| a.ui_events(window, &[UiEvent::Pressed(3)])).unwrap();
    assert!(!names(&mut app).contains("|milk"));
}

/// The state survives a save and a start from it, as across an update.
#[test]
fn the_state_is_saved_and_started_from() {
    let (mut app, window) = started(capp1::TEMPLATE);
    in_call(&mut app, |a| a.ui_events(window, &[UiEvent::Pressed(3)])).unwrap();
    let saved = in_call(&mut app, |a| a.export(0)).unwrap().expect("saved");
    let LiveInstance::V2(mut next) =
        wasmhost::load_live(&built(capp1::TEMPLATE).artifact, LIMITS, None).unwrap()
    else {
        panic!("a lifecycle 2 artifact");
    };
    assert_eq!(in_call(&mut next, |a| a.import(&saved, 1)), Ok(0));
    assert!(names(&mut next).contains("Count: 1"));
    // Another schema is refused, and so is more than KAI_STATE_MAX bytes.
    assert_eq!(in_call(&mut next, |a| a.import(&saved, 2)), Ok(1));
    assert_eq!(in_call(&mut next, |a| a.import(&[0; 4097], 1)), Ok(2));
}

/// A state over `KAI_STATE_MAX` does not build, and says why; the sources
/// the artifact keeps are the app's own.
#[test]
fn a_state_too_big_is_refused_at_build() {
    let big = capp1::TEMPLATE.replace(
        "struct state { I64 count; };",
        "struct state { I64 count; char pad[4096]; };",
    );
    let files = [("main.c", big.as_bytes())];
    let mut budget = Budget::new(&Clock, u64::MAX / 4);
    let build = Build {
        name: "kaiapp",
        version: "0.0.1",
        kind: Kind::Window,
        files: &files,
    };
    let refused = build::build(compiler(), &build, &mut budget).expect_err("too big");
    let why = refused.text().unwrap();
    assert!(
        why.contains("struct_state_is_over_KAI_STATE_MAX_bytes"),
        "{why}"
    );
    let built = built(capp1::TEMPLATE);
    let parsed = livecore::artifact::parse(&built.artifact).unwrap();
    let kept = livecore::artifact::source_files(parsed.source).unwrap();
    assert_eq!(kept, [("main.c", capp1::TEMPLATE.as_bytes())]);
}

/// The names the pre-check keeps variables from are kaiapp.h's calls.
#[test]
fn the_calls_list_names_every_call() {
    for (call, _) in capp1::CALLS {
        let name = call.split('(').next().unwrap().rsplit(' ').next().unwrap();
        assert!(capp1::KAIAPP_CALLS.contains(&name), "{name}");
    }
    assert_eq!(capp1::KAIAPP_CALLS.len(), capp1::CALLS.len());
}

/// Each call the card shows is declared so in kaiapp.h.
#[test]
fn the_cards_calls_are_kaiapp_hs() {
    let header = std::str::from_utf8(KAIAPP_H).unwrap();
    for (call, _) in capp1::CALLS {
        assert!(header.contains(call), "{call}");
    }
}

/// The card stands under the 980-token bar on its own; the token-count job
/// has the real count.
#[test]
fn the_card_fits_its_budget() {
    let card = capp1::render();
    let estimate = pack::estimate_tokens(&card);
    assert!(estimate < pack::PACK_ALARM, "{estimate} estimated tokens");
}
