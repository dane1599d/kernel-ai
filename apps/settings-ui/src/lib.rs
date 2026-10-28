//! The Settings app's window on `kai:ui` (ADR 0005 section 4; P4 task 7): tabs
//! for its pages, and the page under them. As Notes' window does, it rebuilds
//! the tree whole from the model after each event and [`diff`]s it against the
//! last one. A choice comes back from [`on_ui`] as the model's
//! [`Intent`], which the component carries out through the agent's rows, so the
//! window itself changes nothing in the OS.

#![no_std]

extern crate alloc;

mod tree;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use comp_settings::files::{Appearance, DAY_MINUTES};
use comp_settings::{Intent, Page, Settings};
pub use tree::{Change, Entry, diff};
use tree::{entry, label, row, wide};
use uikit::style::{Tone, TypeStyle};
use uikit::widget::{Axis, Layout, NodeId, ROOT, UiEvent, Value, Widget};

pub const COLUMN: NodeId = 1;
pub const TABS: NodeId = 2;
pub const PAGE: NodeId = 3;
pub const HEADING: NodeId = 4;
// Appearance.
pub const THEME: NodeId = 10;
pub const LEGIBLE: NodeId = 11;
pub const MOTION: NodeId = 12;
// What apps can do.
pub const APP_LIST: NodeId = 20;
pub const APP_ITEM: NodeId = 1000;
pub const ACCESS: NodeId = 2000;
pub const REVOKE: NodeId = 3000;
// Rules and profiles.
pub const RULES: NodeId = 30;
pub const RULE: NodeId = 4000;
pub const REMOVE: NodeId = 5000;
pub const PROFILES: NodeId = 31;
pub const PROFILE_ITEM: NodeId = 6000;
// Keep running.
pub const KEEP: NodeId = 7000;
// Updates.
pub const WINDOW_ON: NodeId = 40;
pub const START: NodeId = 41;
pub const END: NodeId = 42;
// About.
pub const ABOUT: NodeId = 8000;

/// Most rows a page shows of one kind, so node ids stay apart.
pub const MAX_ROWS: usize = 999;
/// The window's size.
pub const SIZE: (u16, u16) = (640, 460);
/// The restart window a switch turns on with: 02:00 to 05:00.
pub const DEFAULT_WINDOW: (u16, u16) = (120, 300);

/// What the window shows that the model does not hold.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub page: usize,
    /// The app chosen on "What apps can do", by its place in the list.
    pub app: Option<usize>,
}

/// `hh:mm` for minute `m` of the day.
pub fn clock(m: u16) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// The window's tree for `settings` as `view` shows it.
pub fn build(settings: &Settings, view: &View) -> Vec<Entry> {
    let page = Page::ALL[view.page.min(Page::ALL.len() - 1)];
    let mut tree = alloc::vec![
        entry(
            COLUMN,
            ROOT,
            Layout::FILL,
            Widget::Stack { axis: Axis::Column }
        ),
        entry(
            TABS,
            COLUMN,
            wide(),
            Widget::Tabs {
                labels: Page::ALL.iter().map(|p| String::from(p.title())).collect(),
                selected: view.page as u32,
            },
        ),
        entry(
            PAGE,
            COLUMN,
            tree::column(12, 8),
            Widget::Stack { axis: Axis::Column }
        ),
        entry(
            HEADING,
            PAGE,
            wide(),
            label(page.title(), TypeStyle::Heading, Tone::Normal)
        ),
    ];
    match page {
        Page::Appearance => appearance(settings, &mut tree),
        Page::Access => access(settings, view, &mut tree),
        Page::Rules => rules(settings, &mut tree),
        Page::Apps => apps(settings, &mut tree),
        Page::Updates => updates(settings, &mut tree),
        Page::About => about(settings, &mut tree),
    }
    tree
}

fn appearance(s: &Settings, tree: &mut Vec<Entry>) {
    let a = &s.appearance;
    tree.push(entry(
        THEME,
        PAGE,
        wide(),
        Widget::Button {
            text: format!("Theme: {}", a.theme),
            action: Some(String::from("appearance_set")),
        },
    ));
    let toggle = |text: &str, on| Widget::Toggle {
        label: String::from(text),
        on,
    };
    tree.push(entry(
        LEGIBLE,
        PAGE,
        wide(),
        toggle("Legible type", a.legible),
    ));
    tree.push(entry(
        MOTION,
        PAGE,
        wide(),
        toggle("Reduce motion", a.reduced_motion),
    ));
}

fn access(s: &Settings, view: &View, tree: &mut Vec<Entry>) {
    if s.access.is_empty() {
        tree.push(entry(
            ACCESS,
            PAGE,
            wide(),
            label(
                "No app has been given anything.",
                TypeStyle::Body,
                Tone::Muted,
            ),
        ));
        return;
    }
    let selected = view.app.filter(|&i| i < s.access.len());
    tree.push(entry(
        APP_LIST,
        PAGE,
        wide(),
        Widget::ItemList {
            label: String::from("Apps"),
            selected: selected.map(|i| i as u32),
        },
    ));
    for (i, app) in s.access.iter().take(MAX_ROWS).enumerate() {
        tree.push(entry(
            APP_ITEM + i as u32,
            APP_LIST,
            wide(),
            label(&app.app, TypeStyle::Body, Tone::Normal),
        ));
    }
    let Some(app) = selected.map(|i| &s.access[i]) else {
        return;
    };
    for (i, x) in app.access.iter().take(MAX_ROWS).enumerate() {
        let at = ACCESS + 2 * i as u32;
        tree.push(entry(at, PAGE, wide(), row()));
        tree.push(entry(
            at + 1,
            at,
            wide(),
            label(
                &format!("{} ({})", x.what, x.why),
                TypeStyle::Body,
                Tone::Normal,
            ),
        ));
        tree.push(entry(
            REVOKE + i as u32,
            at,
            Layout::FIT,
            Widget::Button {
                text: String::from("Revoke"),
                action: None,
            },
        ));
    }
}

fn rules(s: &Settings, tree: &mut Vec<Entry>) {
    tree.push(entry(
        RULES,
        PAGE,
        wide(),
        label("Standing rules", TypeStyle::Label, Tone::Normal),
    ));
    if s.rules.is_empty() {
        tree.push(entry(
            RULE,
            PAGE,
            wide(),
            label("None: every ask still asks.", TypeStyle::Body, Tone::Muted),
        ));
    }
    for (i, r) in s.rules.iter().take(MAX_ROWS).enumerate() {
        let at = RULE + 1 + 2 * i as u32;
        tree.push(entry(at, PAGE, wide(), row()));
        tree.push(entry(
            at + 1,
            at,
            wide(),
            label(&r.text, TypeStyle::Body, Tone::Normal),
        ));
        tree.push(entry(
            REMOVE + i as u32,
            at,
            Layout::FIT,
            Widget::Button {
                text: String::from("Remove"),
                action: Some(String::from("rules_remove")),
            },
        ));
    }
    let selected = s
        .profile
        .as_ref()
        .and_then(|p| s.profiles.iter().position(|n| n == p))
        .map(|i| i as u32);
    tree.push(entry(
        PROFILES,
        PAGE,
        wide(),
        Widget::ItemList {
            label: String::from("Profiles"),
            selected,
        },
    ));
    for (i, name) in s.profiles.iter().take(MAX_ROWS).enumerate() {
        tree.push(entry(
            PROFILE_ITEM + i as u32,
            PROFILES,
            wide(),
            label(name, TypeStyle::Body, Tone::Normal),
        ));
    }
}

fn apps(s: &Settings, tree: &mut Vec<Entry>) {
    if s.nappable.is_empty() {
        tree.push(entry(
            KEEP,
            PAGE,
            wide(),
            label("No app naps yet.", TypeStyle::Body, Tone::Muted),
        ));
    }
    for (i, app) in s.nappable.iter().take(MAX_ROWS).enumerate() {
        tree.push(entry(
            KEEP + 1 + i as u32,
            PAGE,
            wide(),
            Widget::Toggle {
                label: format!("Keep {app} running"),
                on: s.apps.keeps(app),
            },
        ));
    }
}

fn updates(s: &Settings, tree: &mut Vec<Entry>) {
    let window = s.updates.restart_window;
    tree.push(entry(
        WINDOW_ON,
        PAGE,
        wide(),
        Widget::Toggle {
            label: String::from("Restart for an update by itself, in a time window"),
            on: window.is_some(),
        },
    ));
    let Some((start, end)) = window else {
        return;
    };
    let slider = |text: String, value: u16| Widget::Slider {
        label: text,
        min: 0,
        max: i32::from(DAY_MINUTES - 15),
        step: 15,
        value: i32::from(value),
    };
    tree.push(entry(
        START,
        PAGE,
        wide(),
        slider(format!("From {}", clock(start)), start),
    ));
    tree.push(entry(
        END,
        PAGE,
        wide(),
        slider(format!("Until {}", clock(end)), end),
    ));
}

fn about(s: &Settings, tree: &mut Vec<Entry>) {
    let a = &s.about;
    let lines = [
        format!("Kernel AI {} (commit {})", a.version, a.commit),
        format!("Licence: {}", a.licence),
        format!("Source: {}", a.source),
        a.notices.clone(),
    ];
    for (i, text) in lines.iter().enumerate() {
        tree.push(entry(
            ABOUT + i as u32,
            PAGE,
            wide(),
            label(text, TypeStyle::Body, Tone::Normal),
        ));
    }
}

/// `id` as the `i`th of the rows starting at `base`, if it is one.
fn nth(id: NodeId, base: NodeId) -> Option<usize> {
    id.checked_sub(base)
        .map(|i| i as usize)
        .filter(|&i| i < MAX_ROWS)
}

/// Handles one event: changes `view` for a page or a selection, and answers a
/// choice with the intent to carry out. True if the window must be drawn again.
pub fn on_ui(settings: &Settings, view: &mut View, e: &UiEvent) -> (bool, Option<Intent>) {
    match e {
        UiEvent::Changed(TABS, Value::Index(i)) if (*i as usize) < Page::ALL.len() => {
            *view = View {
                page: *i as usize,
                app: None,
            };
            (true, None)
        }
        UiEvent::Changed(APP_LIST, Value::Index(i)) => {
            view.app = Some(*i as usize).filter(|&i| i < settings.access.len());
            (true, None)
        }
        UiEvent::Pressed(THEME) => (false, Some(settings.next_theme())),
        UiEvent::Changed(LEGIBLE | MOTION, Value::On(on)) => {
            let mut a: Appearance = settings.appearance.clone();
            if matches!(e, UiEvent::Changed(LEGIBLE, _)) {
                a.legible = *on;
            } else {
                a.reduced_motion = *on;
            }
            let changed = a != settings.appearance;
            (false, changed.then_some(Intent::Appearance(a)))
        }
        UiEvent::Pressed(id) => (false, pressed(settings, view, *id)),
        UiEvent::Changed(PROFILES, Value::Index(i)) => {
            let name = settings.profiles.get(*i as usize).cloned();
            let changed = name.is_some() && name != settings.profile;
            (false, name.filter(|_| changed).map(Intent::UseProfile))
        }
        UiEvent::Changed(WINDOW_ON, Value::On(on)) => {
            let w = on.then_some(DEFAULT_WINDOW);
            (false, settings.restart_window(w))
        }
        UiEvent::Changed(id @ (START | END), Value::Number(n)) => {
            let (Some((start, end)), Ok(n)) = (settings.updates.restart_window, u16::try_from(*n))
            else {
                return (false, None);
            };
            let w = if *id == START { (n, end) } else { (start, n) };
            (false, settings.restart_window(Some(w)))
        }
        UiEvent::Changed(id, Value::On(on)) => {
            let app = nth(*id, KEEP + 1).and_then(|i| settings.nappable.get(i));
            (false, app.and_then(|a| settings.keep_running(a, *on)))
        }
        _ => (false, None),
    }
}

fn pressed(settings: &Settings, view: &View, id: NodeId) -> Option<Intent> {
    if let Some(i) = nth(id, REVOKE) {
        let app = settings.access.get(view.app?)?;
        return settings.revoke(app.access.get(i)?.grant);
    }
    let rule = settings.rules.get(nth(id, REMOVE)?)?;
    Some(Intent::RemoveRule(rule.id))
}
