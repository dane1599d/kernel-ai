//! The Settings window on the host, through a real `uikit::Window`: every
//! page's tree is one `uikit` takes, switching pages sends only what changed,
//! and each control gives the model's intent.

use comp_settings::access::by_app;
use comp_settings::files::Apps;
use comp_settings::{About, Intent, Page, RuleLine, RuleRef, Settings};
use comp_settings_ui::{
    APP_LIST, Change, END, Entry, KEEP, LEGIBLE, PROFILES, REMOVE, REVOKE, SIZE, START, TABS,
    THEME, View, WINDOW_ON, build, clock, diff, on_ui, redraw,
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
        for app in [None, Some("notes"), Some("browser"), Some("gone")] {
            let mut view = View::page(page);
            view.app = app.map(String::from);
            let tree = build(&s, &mut view);
            let shown = names(&tree);
            assert!(
                shown.iter().any(|n| n == Page::ALL[page].title()),
                "{shown:?}"
            );
        }
    }
    let about = names(&build(&s, &mut View::page(5)));
    assert!(about.iter().any(|n| n == "Licence: MPL-2.0"), "{about:?}");
}

#[test]
fn switching_pages_sends_only_what_changed() {
    let s = settings();
    let mut view = View::default();
    let first = build(&s, &mut view);
    let (redraw, intent) = on_ui(&s, &mut view, &UiEvent::Changed(TABS, Value::Index(3)));
    assert!(redraw && intent.is_none());
    let second = build(&s, &mut view);
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
            .any(|c| matches!(c, Change::Put(e) if e.id == KEEP))
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
    let mut view = View::page(1);
    build(&s, &mut view);
    assert_eq!(on_ui(&s, &mut view, &UiEvent::Pressed(REVOKE)).1, None);
    on_ui(&s, &mut view, &UiEvent::Changed(APP_LIST, Value::Index(1)));
    assert_eq!(view.app.as_deref(), Some("notes"));
    build(&s, &mut view);
    assert_eq!(
        on_ui(&s, &mut view, &UiEvent::Pressed(REVOKE)).1,
        Some(Intent::Revoke(41))
    );
    assert_eq!(on_ui(&s, &mut view, &UiEvent::Pressed(REVOKE + 1)).1, None);
    on_ui(&s, &mut view, &UiEvent::Changed(APP_LIST, Value::Index(9)));
    assert_eq!(view.app, None);
}

const MORE_GRANTS: &str = "kai-grants 1\n\
grant 41 app=notes kind=dir object=/home/user/notes rights=read+write from=picker life=persistent signer=- made=2026-11-02T10:14:00Z\n\
grant 42 app=browser kind=host object=example.org rights=connect from=request:7 life=session signer=- made=2026-11-02T10:15:00Z\n\
grant 43 app=notes kind=dir object=/home/user/drafts rights=read from=picker life=persistent signer=- made=2026-11-02T10:16:00Z\n";

/// A press that comes again after its row went (a double-click, a repeated
/// Enter, the agent retrying) names nothing, never the row that moved up.
#[test]
fn a_second_press_after_a_revoke_never_names_another_grant() {
    let mut s = Settings {
        access: by_app(&kaigrants::parse(MORE_GRANTS).unwrap()),
        ..settings()
    };
    let mut view = View::page(1);
    build(&s, &mut view);
    let browser = s.access.iter().position(|a| a.app == "browser").unwrap();
    on_ui(
        &s,
        &mut view,
        &UiEvent::Changed(APP_LIST, Value::Index(browser as u32)),
    );
    build(&s, &mut view);
    let revoke = on_ui(&s, &mut view, &UiEvent::Pressed(REVOKE)).1;
    assert_eq!(revoke, Some(Intent::Revoke(42)));
    s.applied(&revoke.unwrap());
    let tree = build(&s, &mut view);
    // Browser is gone, so nothing is chosen, and the same press does nothing.
    assert!(!tree.iter().any(|e| e.id == REVOKE));
    assert_eq!(on_ui(&s, &mut view, &UiEvent::Pressed(REVOKE)).1, None);
    // Within one app: the first grant's button names only that grant.
    let notes = s.access.iter().position(|a| a.app == "notes").unwrap();
    on_ui(
        &s,
        &mut view,
        &UiEvent::Changed(APP_LIST, Value::Index(notes as u32)),
    );
    build(&s, &mut view);
    let first = on_ui(&s, &mut view, &UiEvent::Pressed(REVOKE)).1.unwrap();
    let Intent::Revoke(gone) = first else {
        panic!("{first:?}");
    };
    s.applied(&first);
    build(&s, &mut view);
    assert_eq!(view.app.as_deref(), Some("notes"));
    assert_eq!(on_ui(&s, &mut view, &UiEvent::Pressed(REVOKE)).1, None);
    let other = if gone == 41 { 43 } else { 41 };
    assert_eq!(
        on_ui(&s, &mut view, &UiEvent::Pressed(REVOKE + 1)).1,
        Some(Intent::Revoke(other))
    );
}

#[test]
fn a_second_press_after_a_rule_is_removed_never_names_another_rule() {
    let mut s = settings();
    let mut view = View::page(2);
    build(&s, &mut view);
    let remove = on_ui(&s, &mut view, &UiEvent::Pressed(REMOVE)).1;
    assert_eq!(remove, Some(Intent::RemoveRule(RuleRef::Rule(3))));
    s.rules.remove(0);
    build(&s, &mut view);
    assert_eq!(on_ui(&s, &mut view, &UiEvent::Pressed(REMOVE)).1, None);
    assert_eq!(
        on_ui(&s, &mut view, &UiEvent::Pressed(REMOVE + 1)).1,
        Some(Intent::RemoveRule(RuleRef::Instruction(2)))
    );
}

/// `uikit` moves a slider or a toggle itself; when the model does not follow,
/// the next draw puts the control back to what the model holds.
#[test]
fn a_refused_change_puts_the_control_back() {
    let mut s = settings();
    s.updates.restart_window = Some((120, 300));
    let mut view = View::page(4);
    let old = build(&s, &mut view);
    let (draw, intent) = on_ui(&s, &mut view, &UiEvent::Changed(START, Value::Number(300)));
    assert!(draw && intent.is_none());
    let new = build(&s, &mut view);
    let changes = redraw(&old, &new, &mut view);
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, Change::Put(e) if e.id == START && e.widget == old.iter().find(|o| o.id == START).unwrap().widget)),
        "{changes:?}"
    );
    // Only once: the draw after that sends nothing.
    assert!(redraw(&new, &build(&s, &mut view), &mut view).is_empty());
}

#[test]
fn rules_profiles_and_keep_running_give_their_intents() {
    let s = settings();
    let mut view = View::page(2);
    build(&s, &mut view);
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
    let mut view = View::page(3);
    build(&s, &mut view);
    assert_eq!(
        on_ui(&s, &mut view, &UiEvent::Changed(KEEP, Value::On(true))).1,
        Some(Intent::KeepRunning {
            app: "files".into(),
            on: true
        })
    );
    assert_eq!(
        on_ui(&s, &mut view, &UiEvent::Changed(KEEP + 1, Value::On(true))).1,
        None
    );
}

#[test]
fn the_restart_window_turns_on_and_moves_in_quarter_hours() {
    let mut s = settings();
    let mut view = View::page(4);
    let (_, on) = on_ui(&s, &mut view, &UiEvent::Changed(WINDOW_ON, Value::On(true)));
    assert_eq!(on, Some(Intent::RestartWindow(Some((120, 300)))));
    s.applied(&on.unwrap());
    let tree = names(&build(&s, &mut view));
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

#[test]
fn every_page_at_its_limit_has_unique_ids_and_fits_one_window() {
    use comp_settings::access::{Access, AppAccess};
    use comp_settings_ui::MAX_ROWS;
    let n = MAX_ROWS + 5;
    let s = Settings {
        access: vec![AppAccess {
            app: "many".into(),
            access: (0..n as u32)
                .map(|grant| Access {
                    grant,
                    what: format!("Read and write the file /home/user/{grant:0>200}"),
                    why: "test".into(),
                })
                .collect(),
        }],
        rules: (0..n as u32)
            .map(|i| RuleLine {
                id: RuleRef::Rule(i),
                text: format!("Always allow rule {i:0>200}"),
            })
            .collect(),
        profiles: (0..n).map(|i| format!("p{i}")).collect(),
        nappable: (0..n).map(|i| format!("a{i}")).collect(),
        ..Settings::default()
    };
    for page in 0..Page::ALL.len() {
        let mut view = View::page(page);
        view.app = Some("many".into());
        let tree = build(&s, &mut view);
        // uikit takes it whole: within 4,096 nodes and 256 KiB.
        let shown = names(&tree);
        if page == 1 || page == 2 {
            assert!(
                shown.iter().any(|n| n.ends_with("more not shown")),
                "page {page}"
            );
        }
        let mut ids: Vec<u32> = tree.iter().map(|e| e.id).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "page {page} reuses an id");
        // Every parent is a node put before it.
        for (i, e) in tree.iter().enumerate().skip(1) {
            assert!(
                tree[..i].iter().any(|p| p.id == e.parent),
                "page {page}: {} before its parent {}",
                e.id,
                e.parent
            );
        }
    }
}
