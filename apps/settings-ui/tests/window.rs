//! The Settings window on the host, through a real `uikit::Window`: every
//! page's tree is one `uikit` takes, switching pages sends only what changed,
//! and each control gives the model's intent.

use comp_settings::access::by_app;
use comp_settings::files::Apps;
use comp_settings::{About, Intent, Page, RuleLine, RuleRef, Settings};
use comp_settings_ui::{
    APP_LIST, Change, END, Entry, KEEP, LEGIBLE, PROFILES, REMOVE, REVOKE, SIZE, START, TABS,
    THEME, View, WINDOW_ON, build, clock, diff, on_ui,
};
use uikit::layout::FixedAdvance;
use uikit::widget::{UiEvent, Value};
use uikit::window::Window;

const GRANTS: &str = "kai-grants 1\n\
grant 41 app=notes kind=dir object=/home/user/notes rights=read+write from=picker life=persistent signer=- made=2026-11-02T10:14:00Z\n\
grant 42 app=browser kind=host object=example.org rights=connect from=request:7 life=session signer=- made=2026-11-02T10:15:00Z\n";

fn settings() -> Settings {
    let mut apps = Apps::default();
    apps.set("notes", true);
    Settings {
        access: by_app(&kaigrants::parse(GRANTS).unwrap()),
        rules: vec![
            RuleLine {
                id: RuleRef::Rule(3),
                text: "Always stop build-3 when asked".into(),
            },
            RuleLine {
                id: RuleRef::Instruction(2),
                text: "When a build finishes, tell me".into(),
            },
        ],
        profiles: vec!["normal".into(), "focus".into()],
        profile: Some("normal".into()),
        nappable: vec!["files".into(), "notes".into()],
        apps,
        about: About {
            version: "0.1.0-dev".into(),
            commit: "abc".into(),
            licence: "MPL-2.0".into(),
            source: "https://github.com/dane1599d/kernel-ai".into(),
            notices: "54 crates; see NOTICES.txt".into(),
        },
        ..Settings::default()
    }
}

/// Puts a whole tree into a fresh window, as the component's first draw does,
/// and returns the names the window's accessibility tree gives.
fn names(tree: &[Entry]) -> Vec<String> {
    let mut w = Window::new("Settings", SIZE.0, SIZE.1);
    for e in tree {
        w.put(e.id, e.parent, e.layout, e.widget.clone())
            .unwrap_or_else(|err| panic!("{err:?} putting {e:?}"));
    }
    w.commit(&FixedAdvance)
        .a11y
        .into_iter()
        .map(|n| n.name)
        .collect()
}

#[test]
fn every_page_is_a_tree_uikit_takes() {
    let s = settings();
    for page in 0..Page::ALL.len() {
        for app in [None, Some(0), Some(1)] {
            let tree = build(&s, &View { page, app });
            let shown = names(&tree);
            assert!(
                shown.iter().any(|n| n == Page::ALL[page].title()),
                "{shown:?}"
            );
        }
    }
    let about = names(&build(&s, &View { page: 5, app: None }));
    assert!(about.iter().any(|n| n == "Licence: MPL-2.0"), "{about:?}");
}

#[test]
fn switching_pages_sends_only_what_changed() {
    let s = settings();
    let mut view = View::default();
    let first = build(&s, &view);
    let (redraw, intent) = on_ui(&s, &mut view, &UiEvent::Changed(TABS, Value::Index(3)));
    assert!(redraw && intent.is_none());
    let second = build(&s, &view);
    let changes = diff(&first, &second);
    // The tabs and the page's own nodes change; the column stays.
    assert!(
        !changes
            .iter()
            .any(|c| matches!(c, Change::Put(e) if e.id == comp_settings_ui::COLUMN))
    );
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, Change::Put(e) if e.id == KEEP + 1))
    );
    // Applying the changes to the first tree gives the second.
    let mut w = Window::new("Settings", SIZE.0, SIZE.1);
    for e in &first {
        w.put(e.id, e.parent, e.layout, e.widget.clone()).unwrap();
    }
    for c in changes {
        match c {
            Change::Put(e) => w.put(e.id, e.parent, e.layout, e.widget).unwrap(),
            Change::Remove(id) => w.remove(id).unwrap(),
        }
    }
    let applied: Vec<String> = w
        .commit(&FixedAdvance)
        .a11y
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert_eq!(applied, names(&second));
}

#[test]
fn appearance_controls_give_appearance_set() {
    let s = settings();
    let mut view = View::default();
    let (_, intent) = on_ui(&s, &mut view, &UiEvent::Pressed(THEME));
    assert!(matches!(intent, Some(Intent::Appearance(ref a)) if a.theme == look::THEMES[1].name));
    let (_, intent) = on_ui(&s, &mut view, &UiEvent::Changed(LEGIBLE, Value::On(true)));
    assert!(matches!(intent, Some(Intent::Appearance(ref a)) if a.legible));
    let (_, intent) = on_ui(&s, &mut view, &UiEvent::Changed(LEGIBLE, Value::On(false)));
    assert_eq!(intent, None);
}

#[test]
fn revoke_needs_a_chosen_app_and_names_its_grant() {
    let s = settings();
    let mut view = View { page: 1, app: None };
    assert_eq!(on_ui(&s, &mut view, &UiEvent::Pressed(REVOKE)).1, None);
    on_ui(&s, &mut view, &UiEvent::Changed(APP_LIST, Value::Index(1)));
    assert_eq!(view.app, Some(1));
    assert_eq!(
        on_ui(&s, &mut view, &UiEvent::Pressed(REVOKE)).1,
        Some(Intent::Revoke(41))
    );
    assert_eq!(on_ui(&s, &mut view, &UiEvent::Pressed(REVOKE + 1)).1, None);
    on_ui(&s, &mut view, &UiEvent::Changed(APP_LIST, Value::Index(9)));
    assert_eq!(view.app, None);
}

#[test]
fn rules_profiles_and_keep_running_give_their_intents() {
    let s = settings();
    let mut view = View { page: 2, app: None };
    assert_eq!(
        on_ui(&s, &mut view, &UiEvent::Pressed(REMOVE)).1,
        Some(Intent::RemoveRule(RuleRef::Rule(3)))
    );
    assert_eq!(
        on_ui(&s, &mut view, &UiEvent::Pressed(REMOVE + 1)).1,
        Some(Intent::RemoveRule(RuleRef::Instruction(2)))
    );
    assert_eq!(
        on_ui(&s, &mut view, &UiEvent::Changed(PROFILES, Value::Index(1))).1,
        Some(Intent::UseProfile("focus".into()))
    );
    assert_eq!(
        on_ui(&s, &mut view, &UiEvent::Changed(PROFILES, Value::Index(0))).1,
        None
    );
    assert_eq!(
        on_ui(&s, &mut view, &UiEvent::Changed(KEEP + 1, Value::On(true))).1,
        Some(Intent::KeepRunning {
            app: "files".into(),
            on: true
        })
    );
    assert_eq!(
        on_ui(&s, &mut view, &UiEvent::Changed(KEEP + 2, Value::On(true))).1,
        None
    );
}

#[test]
fn the_restart_window_turns_on_and_moves_in_quarter_hours() {
    let mut s = settings();
    let mut view = View { page: 4, app: None };
    let (_, on) = on_ui(&s, &mut view, &UiEvent::Changed(WINDOW_ON, Value::On(true)));
    assert_eq!(on, Some(Intent::RestartWindow(Some((120, 300)))));
    s.applied(&on.unwrap());
    let tree = names(&build(&s, &view));
    assert!(tree.iter().any(|n| n.contains(&clock(120))), "{tree:?}");
    assert_eq!(
        on_ui(&s, &mut view, &UiEvent::Changed(END, Value::Number(360))).1,
        Some(Intent::RestartWindow(Some((120, 360))))
    );
    // A start equal to the end is no window, and a negative minute is ignored.
    assert_eq!(
        on_ui(&s, &mut view, &UiEvent::Changed(START, Value::Number(300))).1,
        None
    );
    assert_eq!(
        on_ui(&s, &mut view, &UiEvent::Changed(START, Value::Number(-15))).1,
        None
    );
    assert_eq!(
        on_ui(
            &s,
            &mut view,
            &UiEvent::Changed(WINDOW_ON, Value::On(false))
        )
        .1,
        Some(Intent::RestartWindow(None))
    );
}
