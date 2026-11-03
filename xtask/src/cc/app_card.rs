//! The `c-app-1` card's host tests (plan "Kai builds apps", piece 2): the
//! template and every example `cc build` as window apps and run in wasmhost
//! as comphost starts them, each call the card shows is `kaiapp.h`'s, and the
//! card fits its budget.

use super::app_run::{self, in_call};
use ccserve::build::{self, Built, KAIAPP_H, Kind};
use livecore::Instance;
use llmproto::pack::{self, capp1};
use uikit::widget::{UiEvent, Value};
use wasmhost::CoreInstance;

fn try_build(main: &str) -> Result<Built, build::BuildError> {
    let (kind, built) = app_run::build_main("kaiapp", "0.0.1", main)?;
    assert_eq!(kind, Kind::Window);
    Ok(built)
}

/// `main` built as the app cc would build it, as the shell sends it.
fn built(main: &str) -> Built {
    try_build(main).unwrap_or_else(|e| panic!("{e:?}\n{main}"))
}

fn loaded(main: &str) -> CoreInstance {
    app_run::load(&built(main).artifact).unwrap()
}

/// The app's own state, as comphost exports it.
fn saved(app: &mut CoreInstance) -> Vec<u8> {
    app_run::saved(app).unwrap()
}

/// The app, started as comphost starts a version. Its window's id.
fn started(main: &str) -> (CoreInstance, u32) {
    app_run::start(&built(main).artifact).unwrap_or_else(|why| panic!("{why}\n{main}"))
}

/// The names the last frame shows, in node order.
fn shown(app: &mut CoreInstance) -> Vec<String> {
    app_run::last_frame(app)
        .expect("a frame")
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
        let limits = llmproto::script::APP_LIMITS;
        assert!(
            llmproto::script::check_text_within(main, limits).is_ok(),
            "{request}"
        );
    }
}

/// The shopping list keeps a field's text, adds it on Enter and clears.
#[test]
fn the_list_example_keeps_what_was_typed() {
    let (_, main, _) = capp1::EXAMPLES[0];
    let (mut app, window) = started(main);
    let typed = UiEvent::Changed(1, Value::Text("milk".into()));
    in_call(&mut app, |a| {
        a.ui_events(window, &[typed, UiEvent::Submitted(1)])
    })
    .unwrap();
    let after = names(&mut app);
    assert!(after.contains("|milk"), "{after}");
    in_call(&mut app, |a| a.ui_events(window, &[UiEvent::Pressed(2)])).unwrap();
    assert!(!names(&mut app).contains("|milk"));
}

/// The state survives a save and a start from it, as across an update. It
/// is CBOR [1, bytes], exactly struct state's size.
#[test]
fn the_state_is_saved_and_started_from() {
    let (mut app, window) = started(capp1::TEMPLATE);
    in_call(&mut app, |a| a.ui_events(window, &[UiEvent::Pressed(3)])).unwrap();
    let state = saved(&mut app);
    assert_eq!(state, [0x82, 0x01, 0x59, 0, 8, 1, 0, 0, 0, 0, 0, 0, 0]);
    let mut next = loaded(capp1::TEMPLATE);
    assert_eq!(next.import(&state, 1), Ok(0));
    assert!(names(&mut next).contains("Count: 1"));
    // Another schema is refused, and so is a state that is not one.
    assert_eq!(next.import(&state, 2), Ok(1));
    assert_eq!(next.import(&[0; 13], 1), Ok(2));
    assert_eq!(next.import(&state[..12], 1), Ok(2));
}

/// A version whose struct state is another size starts afresh from init
/// instead of reading the old bytes as its own.
#[test]
fn a_rebuilt_state_of_another_size_starts_afresh() {
    let (mut app, window) = started(capp1::TEMPLATE);
    in_call(&mut app, |a| a.ui_events(window, &[UiEvent::Pressed(3)])).unwrap();
    let state = saved(&mut app);
    let grown = capp1::TEMPLATE.replace(
        "struct state { I64 count; };",
        "struct state { I64 step; I64 count; };",
    );
    let mut next = loaded(&grown);
    assert_eq!(next.import(&state, 1), Ok(0));
    assert!(names(&mut next).contains("Count: 0"));
    assert_eq!(saved(&mut next)[3..5], [0, 16]);
}

/// Text cut at a limit ends at a whole character: the view takes only UTF-8,
/// and a split character would stop the app.
#[test]
fn text_is_cut_at_whole_characters() {
    // A heading of 255 bytes and a two-byte character across the 256th.
    let long = format!("{}\u{e9}", "a".repeat(255));
    let heading = capp1::TEMPLATE.replace(
        "ui_heading(1, 0, text);",
        &format!("ui_heading(1, 0, \"{long}\");"),
    );
    let (mut app, _) = started(&heading);
    assert!(names(&mut app).contains(&"a".repeat(255)));
    // Format into 64 bytes with a two-byte character across the 64th.
    let (_, main, _) = capp1::EXAMPLES[0];
    let (mut app, window) = started(main);
    let typed = UiEvent::Changed(1, Value::Text(format!("{}\u{e9}x", "b".repeat(62))));
    in_call(&mut app, |a| {
        a.ui_events(window, &[typed, UiEvent::Submitted(1)])
    })
    .unwrap();
    let after = names(&mut app);
    assert!(after.contains(&format!("|{}", "b".repeat(62))), "{after}");
}

/// A state over `KAI_STATE_MAX` does not build, and says why, wherever it
/// is defined; the sources the artifact keeps are the app's own.
#[test]
fn a_state_too_big_is_refused_at_build() {
    let big = capp1::TEMPLATE.replace(
        "struct state { I64 count; };",
        "struct  state { I64 count; char pad[4096]; };",
    );
    let why = try_build(&big).expect_err("too big").text().unwrap();
    assert!(
        why.contains("struct_state_is_over_KAI_STATE_MAX_bytes"),
        "{why}"
    );
    let built = built(capp1::TEMPLATE);
    let parsed = livecore::artifact::parse(&built.artifact).unwrap();
    let kept = livecore::artifact::source_files(parsed.source).unwrap();
    assert_eq!(kept, [("main.c", capp1::TEMPLATE.as_bytes())]);
}

/// The names the pre-check keeps variables from are kaiapp.h's calls and
/// the size the build defines.
#[test]
fn the_calls_list_names_every_call() {
    for (call, _) in capp1::CALLS {
        let name = call.split('(').next().unwrap().rsplit(' ').next().unwrap();
        assert!(capp1::KAIAPP_CALLS.contains(&name), "{name}");
    }
    assert!(capp1::KAIAPP_CALLS.contains(&"kai_state_size"));
    assert_eq!(capp1::KAIAPP_CALLS.len(), capp1::CALLS.len() + 1);
}

/// Each call the card shows is declared so in kaiapp.h, and the card gives
/// the state's limit.
#[test]
fn the_cards_calls_are_kaiapp_hs() {
    let header = std::str::from_utf8(KAIAPP_H).unwrap();
    for (call, _) in capp1::CALLS {
        assert!(header.contains(call), "{call}");
    }
    assert!(header.contains("#define KAI_STATE_MAX 4096"));
    assert!(capp1::render().contains("at most 4096 bytes"));
}

/// The card stands under the 980-token bar on its own; the token-count job
/// has the real count.
#[test]
fn the_card_fits_its_budget() {
    let card = capp1::render();
    let estimate = capp1::estimate_tokens(&card);
    assert!(
        estimate < pack::PACK_ALARM,
        "{estimate} estimated tokens, {} bytes",
        card.len()
    );
}
