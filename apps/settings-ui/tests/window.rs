//! The Settings window on the host, through a real `uikit::Window`: every
//! page's tree is one `uikit` takes, switching pages sends only what changed,
//! and each control gives the model's intent.

use comp_settings::access;
use comp_settings::files::Apps;
use comp_settings::{About, Intent, Page, RuleLine, RuleRef, Settings};
use comp_settings_ui::{
    APP_LIST, Change, END, Entry, KEEP, LEGIBLE, PROFILES, REMOVE, RESTORE, REVOKE, SIZE, START,
    TABS, THEME, View, WINDOW_ON, build, clock, diff, on_ui, redraw,
};
use uikit::layout::FixedAdvance;
use uikit::widget::{UiEvent, Value};
use uikit::window::Window;

const GRANTS: &str = "notes\tnotes_add\tallowed\nbrowser\tnet_fetch\tallowed\n";

fn settings() -> Settings {
    let mut apps = Apps::default();
    apps.set("notes", true);
    Settings {
        access: access::parse(GRANTS).unwrap(),
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
fn revoke_needs_a_chosen_app_and_names_its_row() {
    let s = settings();
    let mut view = View::page(1);
    build(&s, &mut view);
    assert_eq!(on_ui(&s, &mut view, &UiEvent::Pressed(REVOKE)).1, None);
    on_ui(&s, &mut view, &UiEvent::Changed(APP_LIST, Value::Index(1)));
    assert_eq!(view.app.as_deref(), Some("notes"));
    build(&s, &mut view);
    assert_eq!(
        on_ui(&s, &mut view, &UiEvent::Pressed(REVOKE)).1,
        Some(Intent::Revoke {
            app: "notes".into(),
            row: "notes_add".into()
        })
    );
    assert_eq!(on_ui(&s, &mut view, &UiEvent::Pressed(REVOKE + 1)).1, None);
    // A row that is allowed has no Restore to press.
    assert_eq!(on_ui(&s, &mut view, &UiEvent::Pressed(RESTORE)).1, None);
    on_ui(&s, &mut view, &UiEvent::Changed(APP_LIST, Value::Index(9)));
    assert_eq!(view.app, None);
}

const MORE_GRANTS: &str = "notes\tnotes_add\tallowed\n\
browser\tnet_fetch\tallowed\n\
notes\tfs_write\tallowed\n";

fn pick(s: &Settings, view: &mut View, app: &str) {
    let at = s.access.iter().position(|a| a.app == app).unwrap();
    on_ui(
        s,
        view,
        &UiEvent::Changed(APP_LIST, Value::Index(at as u32)),
    );
    assert_eq!(view.app.as_deref(), Some(app));
    build(s, view);
}

fn press(s: &Settings, view: &mut View, id: u32) -> Option<Intent> {
    on_ui(s, view, &UiEvent::Pressed(id)).1
}

/// A press that comes again after its row changed (a double-click, a repeated
/// Enter, the agent retrying) never does the opposite, and never names another
/// row: Revoke and Restore are separate buttons, and each sits in its row's slot.
#[test]
fn a_second_press_after_a_revoke_never_undoes_it_or_names_another_row() {
    let mut s = Settings {
        access: access::parse(MORE_GRANTS).unwrap(),
        ..settings()
    };
    let mut view = View::page(1);
    build(&s, &mut view);
    pick(&s, &mut view, "browser");
    let revoke = press(&s, &mut view, REVOKE).unwrap();
    assert_eq!(
        revoke,
        Intent::Revoke {
            app: "browser".into(),
            row: "net_fetch".into()
        }
    );
    s.applied(&revoke);
    let tree = build(&s, &mut view);
    // The row stays, withheld: its button is Restore now.
    assert!(!tree.iter().any(|e| e.id == REVOKE));
    assert!(tree.iter().any(|e| e.id == RESTORE));
    assert_eq!(press(&s, &mut view, REVOKE), None);
    let restore = press(&s, &mut view, RESTORE).unwrap();
    assert_eq!(
        restore,
        Intent::Restore {
            app: "browser".into(),
            row: "net_fetch".into()
        }
    );
    s.applied(&restore);
    let tree = build(&s, &mut view);
    assert!(tree.iter().any(|e| e.id == REVOKE));
    assert!(!tree.iter().any(|e| e.id == RESTORE));
    assert_eq!(press(&s, &mut view, RESTORE), None);

    // Within one app: each button names only its own row.
    pick(&s, &mut view, "notes");
    let first = press(&s, &mut view, REVOKE).unwrap();
    let Intent::Revoke { ref row, .. } = first else {
        panic!("{first:?}");
    };
    assert_eq!(row, "notes_add");
    s.applied(&first);
    build(&s, &mut view);
    assert_eq!(press(&s, &mut view, REVOKE), None);
    assert_eq!(press(&s, &mut view, RESTORE + 1), None);
    assert_eq!(
        press(&s, &mut view, REVOKE + 1),
        Some(Intent::Revoke {
            app: "notes".into(),
            row: "fs_write".into()
        })
    );
    assert!(matches!(
        press(&s, &mut view, RESTORE),
        Some(Intent::Restore { .. })
    ));
}

/// The row says whether the app may call it, and the app says nothing the shell
/// did not list.
#[test]
fn a_row_says_allowed_or_withheld() {
    let s = Settings {
        access: access::parse("notes\tnotes_add\tallowed\nnotes\tfs_write\twithheld\n").unwrap(),
        ..Settings::default()
    };
    let mut view = View::page(1);
    view.app = Some("notes".into());
    build(&s, &mut view);
    let tree = build(&s, &mut view);
    let shown = names(&tree);
    assert!(shown.iter().any(|n| n == "notes_add: allowed"), "{shown:?}");
    assert!(shown.iter().any(|n| n == "fs_write: withheld"), "{shown:?}");
    assert!(shown.iter().any(|n| n == "Revoke"), "{shown:?}");
    assert!(shown.iter().any(|n| n == "Restore"), "{shown:?}");
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
            access: (0..n)
                .map(|i| Access {
                    row: format!("row{i}"),
                    allowed: i % 2 == 0,
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
