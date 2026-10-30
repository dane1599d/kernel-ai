//! The Settings component as built for wasm32, run in `wasmhost` with no OS: it
//! opens its window when it starts, asks the system for what its pages show over
//! `kai:settings`, turns a choice into a request, shows the change only once the
//! answer says it is done and puts a refused control back. It is skipped when
//! `cargo build -p comp-settings-window --target wasm32-unknown-unknown
//! --profile component` has not run (CI runs it first).

use appabi::{Func, Ty, Val};
use comp_settings_ui::{APP_LIST, LEGIBLE, RESTORE, REVOKE, TABS, THEME};
use livecore::Instance;
use livecore::txn::{Inputs, Storage};
use uikit::widget::{UiEvent, Value};
use wasmhost::settings::MAX_QUEUED;
use wasmhost::{
    AppEvent, CoreInstance, Grants, Limits, SettingsKind, Update, load_core, load_core_granted,
};

const LIMITS: Limits = Limits {
    fuel_call: 5_000_000,
    fuel_life: 20_000_000,
    memory_bytes: 2 * 1024 * 1024,
};

fn wasm() -> Option<Vec<u8>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/wasm32-unknown-unknown/component/comp_settings_window.wasm"
    );
    let wasm = std::fs::read(path).ok();
    if wasm.is_none() {
        eprintln!("skipped: build comp-settings-window for wasm32 first");
    }
    wasm
}

fn granted() -> Grants {
    Grants {
        settings: true,
        ..Grants::default()
    }
}

fn start(wasm: &[u8]) -> CoreInstance {
    let mut g = load_core_granted(wasm, LIMITS, None, granted()).unwrap();
    // As the host loads it: storage readable, empty.
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

/// What the app has asked the system for since the last look, as
/// `(ticket, kind, request)`.
fn sent(g: &mut CoreInstance) -> Vec<(u32, SettingsKind, String)> {
    g.take_settings()
        .into_iter()
        .map(|r| (r.ticket, r.kind, r.request))
        .collect()
}

fn answer(g: &mut CoreInstance, ticket: u32, outcome: Result<&str, &str>) {
    let outcome = outcome.map(|t| t.as_bytes().to_vec()).map_err(String::from);
    g.settings_reply(ticket, outcome).unwrap();
}

fn open_page() -> Func {
    Func {
        export: String::from("app:settings/actions@1.0.0#open"),
        params: vec![Ty::String],
        result: Some(Ty::Result(
            Some(Box::new(Ty::String)),
            Some(Box::new(Ty::String)),
        )),
    }
}

/// The theme the button says it will leave: `Theme: dark`.
fn theme_of(f: &uikit::Frame) -> String {
    names(f)
        .into_iter()
        .find_map(|n| n.strip_prefix("Theme: ").map(String::from))
        .unwrap_or_else(|| panic!("no theme button in {:?}", names(f)))
}

#[test]
fn without_the_grant_the_app_does_not_stage() {
    let Some(wasm) = wasm() else { return };
    let fault = match load_core(&wasm, LIMITS, None) {
        Err(fault) => fault,
        Ok(_) => panic!("it linked without the grant"),
    };
    assert_eq!(fault.reason, livecore::Reason::Link);
    assert!(fault.detail.contains("settings grant"), "{}", fault.detail);
}

#[test]
fn it_reads_everything_its_pages_show_when_it_starts_and_fills_them_from_the_answers() {
    let Some(mut g) = load() else { return };
    let (_, f) = frame(&mut g);
    assert_eq!(f.title, "Settings");
    // The window opens at once; nothing is asked until the OS says it started.
    assert!(sent(&mut g).is_empty());
    call(&mut g, |g| g.deliver_event(&AppEvent::Started)).unwrap();
    let asked = sent(&mut g);
    let requests: Vec<&str> = asked.iter().map(|(_, _, r)| r.as_str()).collect();
    assert_eq!(
        requests,
        [
            "rules",
            "grants",
            "about",
            "profiles",
            "apps",
            "file appearance",
            "file apps",
            "file updates"
        ]
    );
    assert!(asked.len() * 2 <= MAX_QUEUED, "room is left for more");
    assert!(asked.iter().all(|(_, kind, _)| *kind == SettingsKind::Read));

    // The rules come back; the Rules page lists them.
    let rules = asked[0].0;
    answer(
        &mut g,
        rules,
        Ok("1 allow stop of cache\nS2 on idle -> app_suspend(mail)"),
    );
    let (w, _) = frame(&mut g);
    call(&mut g, |g| {
        g.ui_events(w, &[UiEvent::Changed(TABS, Value::Index(2))])
            .unwrap()
    });
    let n = names(&frame(&mut g).1);
    assert!(n.iter().any(|t| t.contains("allow stop of cache")), "{n:?}");
    assert!(n.iter().any(|t| t.contains("app_suspend(mail)")), "{n:?}");

    // A refused read leaves the page as it was.
    let about = asked[2].0;
    answer(&mut g, about, Err("the shell is not running"));
    frame(&mut g);
}

#[test]
fn a_choice_is_a_request_and_shows_once_the_answer_says_it_is_done() {
    let Some(mut g) = load() else { return };
    let (w, f) = frame(&mut g);
    assert_eq!(theme_of(&f), "dark");

    // The Theme button asks for the next theme; the button still says dark.
    call(&mut g, |g| {
        g.ui_events(w, &[UiEvent::Pressed(THEME)]).unwrap()
    });
    let asked = sent(&mut g);
    assert_eq!(asked.len(), 1, "{asked:?}");
    let (ticket, kind, request) = asked[0].clone();
    assert_eq!(kind, SettingsKind::Change);
    let value = request
        .strip_prefix("appearance-set theme ")
        .unwrap_or_else(|| panic!("{request}"))
        .to_string();
    assert_ne!(value, "dark");
    assert!(
        g.take_ui().is_empty(),
        "nothing is redrawn until it is done"
    );

    // Done: now the button says the new theme.
    answer(&mut g, ticket, Ok("theme changed"));
    assert_eq!(theme_of(&frame(&mut g).1), value);

    // A refused switch (comphost passes a change on only for a real key or
    // click) leaves the page as it was.
    call(&mut g, |g| {
        g.ui_events(w, &[UiEvent::Pressed(THEME)]).unwrap()
    });
    let asked = sent(&mut g);
    assert_eq!(asked.len(), 1);
    answer(&mut g, asked[0].0, Err("a change needs a key or a click"));
    assert_eq!(theme_of(&frame(&mut g).1), value);

    // A toggle is one `appearance-set` too, with on and off for its value.
    call(&mut g, |g| {
        g.ui_events(w, &[UiEvent::Changed(LEGIBLE, Value::On(true))])
            .unwrap()
    });
    let asked = sent(&mut g);
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].2, "appearance-set legible on");
    answer(&mut g, asked[0].0, Ok("legible on"));
    g.take_ui();

    // Moving between pages asks the system for nothing.
    call(&mut g, |g| {
        g.ui_events(w, &[UiEvent::Changed(TABS, Value::Index(5))])
            .unwrap()
    });
    assert!(sent(&mut g).is_empty());
    assert!(names(&frame(&mut g).1).iter().any(|n| n == "About"));

    // Close: the window goes.
    call(&mut g, |g| g.ui_events(w, &[UiEvent::CloseRequested])).unwrap();
    assert_eq!(g.take_ui(), vec![Update::Closed(w)]);
}

#[test]
fn only_a_change_a_persons_input_caused_is_marked_as_theirs() {
    let Some(mut g) = load() else { return };
    let (w, _) = frame(&mut g);
    // A real click, as comphost's input path delivers it.
    call(&mut g, |g| {
        g.ui_events_user(w, &[UiEvent::Pressed(THEME)]).unwrap()
    });
    let real = g.take_settings();
    assert_eq!(real.len(), 1);
    assert_eq!((real[0].kind, real[0].user), (SettingsKind::Change, true));
    // The same press from anything else, such as the agent's `ui_act`.
    call(&mut g, |g| {
        g.ui_events(w, &[UiEvent::Pressed(THEME)]).unwrap()
    });
    let synthetic = g.take_settings();
    assert_eq!(synthetic.len(), 1);
    assert!(!synthetic[0].user);
    // Reads are never held to it.
    call(&mut g, |g| g.deliver_event(&AppEvent::Started)).unwrap();
    assert!(
        g.take_settings()
            .iter()
            .all(|r| r.kind == SettingsKind::Read)
    );
}

#[test]
fn an_apps_effect_row_is_revoked_and_restored_by_app_and_row() {
    let Some(mut g) = load() else { return };
    let (w, _) = frame(&mut g);
    call(&mut g, |g| g.deliver_event(&AppEvent::Started)).unwrap();
    let asked = sent(&mut g);
    assert_eq!(asked[1].2, "grants");
    let rows = "notes\tnotes_add\tallowed\nnotes\tfs_write\twithheld\n";
    answer(&mut g, asked[1].0, Ok(rows));
    g.take_ui();

    // The page of what apps can do, with the one app chosen.
    call(&mut g, |g| {
        g.ui_events(w, &[UiEvent::Changed(TABS, Value::Index(1))])
            .unwrap()
    });
    g.take_ui();
    call(&mut g, |g| {
        g.ui_events(w, &[UiEvent::Changed(APP_LIST, Value::Index(0))])
            .unwrap()
    });
    let shown = names(&frame(&mut g).1);
    assert!(shown.iter().any(|n| n == "notes_add: allowed"), "{shown:?}");
    assert!(shown.iter().any(|n| n == "fs_write: withheld"), "{shown:?}");

    // A real click on Revoke asks for exactly that row, and the row shows as
    // withheld only once the shell says it is done.
    call(&mut g, |g| {
        g.ui_events_user(w, &[UiEvent::Pressed(REVOKE)]).unwrap()
    });
    let asked = g.take_settings();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].request, "grants-revoke notes notes_add");
    assert_eq!((asked[0].kind, asked[0].user), (SettingsKind::Change, true));
    assert!(g.take_ui().is_empty());
    answer(&mut g, asked[0].ticket, Ok("notes notes_add withheld"));
    let shown = names(&frame(&mut g).1);
    assert!(
        shown.iter().any(|n| n == "notes_add: withheld"),
        "{shown:?}"
    );

    // Restore the other one; a refused change puts the page back as it was.
    call(&mut g, |g| {
        g.ui_events(w, &[UiEvent::Pressed(RESTORE + 1)]).unwrap()
    });
    let asked = g.take_settings();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].request, "grants-restore notes fs_write");
    assert!(!asked[0].user, "the agent's press is not a click");
    answer(
        &mut g,
        asked[0].ticket,
        Err("a change needs a key or a click"),
    );
    let shown = names(&frame(&mut g).1);
    assert!(shown.iter().any(|n| n == "fs_write: withheld"), "{shown:?}");

    // A press on the button the row no longer has asks for nothing.
    call(&mut g, |g| {
        g.ui_events_user(w, &[UiEvent::Pressed(REVOKE)]).unwrap()
    });
    assert!(g.take_settings().is_empty());
}

#[test]
fn an_answer_for_a_ticket_it_did_not_ask_changes_nothing() {
    let Some(mut g) = load() else { return };
    let (_, before) = frame(&mut g);
    answer(&mut g, 4242, Ok("theme changed"));
    answer(&mut g, 0, Ok("1 a rule"));
    assert_eq!(theme_of(&before), "dark");
    assert!(sent(&mut g).is_empty());
}

#[test]
fn the_agent_opens_a_page_by_its_word() {
    let Some(mut g) = load() else { return };
    frame(&mut g);
    let r = call(&mut g, |g| {
        Instance::call(g, &open_page(), &[Val::String("About".into())])
    });
    assert_eq!(
        r.unwrap().unwrap(),
        Val::Result(Ok(Some(Box::new(Val::String("Showing About.".into())))))
    );
    assert!(sent(&mut g).is_empty());
    assert!(names(&frame(&mut g).1).iter().any(|n| n == "About"));
    // A word that is not a page is an answer, not a trap.
    let r = call(&mut g, |g| {
        Instance::call(g, &open_page(), &[Val::String("everything".into())])
    });
    assert!(matches!(r.unwrap().unwrap(), Val::Result(Err(_))));
}

#[test]
fn it_comes_back_to_the_page_it_was_showing() {
    let Some(wasm) = wasm() else { return };
    let mut g = start(&wasm);
    let (w, _) = frame(&mut g);
    call(&mut g, |g| {
        g.ui_events(w, &[UiEvent::Changed(TABS, Value::Index(3))])
            .unwrap()
    });
    g.take_ui();
    let saved = Instance::export(&mut g, 0).unwrap().unwrap();
    let mut h = start(&wasm);
    assert_eq!(h.import(&saved, 1), Ok(0));
    let (_, f) = frame(&mut h);
    // Page 3 is "Keep running"; the Appearance page's theme button is gone.
    assert!(!names(&f).iter().any(|n| n.starts_with("Theme: ")));
    // A state it cannot read and another schema are refused, not guessed at.
    assert_eq!(h.import(b"junk", 1), Ok(2));
    assert_eq!(h.import(&saved, 9), Ok(1));
}
