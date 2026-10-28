//! The Settings app's window on `kai:ui` (ADR 0005 section 4; P4 task 7): tabs
//! for its pages, and the page under them. As Notes' window does, it rebuilds
//! the tree whole from the model after each event and [`diff`]s it against the
//! last one. A choice comes back from [`on_ui`] as the model's
//! [`Intent`](comp_settings::Intent), which the component carries out through the agent's rows, so the
//! window itself changes nothing in the OS.

#![no_std]

extern crate alloc;

mod events;
mod rows;
mod tree;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use comp_settings::files::DAY_MINUTES;
use comp_settings::{Page, RuleRef, Settings};
pub use events::{on_ui, redraw};
use rows::{Room, Slots};
pub use tree::{Change, Entry, diff};
use tree::{entry, label, row, wide};
use uikit::style::{Tone, TypeStyle};
use uikit::widget::{Axis, Layout, NodeId, ROOT, Widget};

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
pub const NO_ACCESS: NodeId = 21;
pub const MORE_ACCESS: NodeId = 22;
pub const MORE_APPS: NodeId = 23;
// Rows of one kind take a block of 1,000 ids each, and a row's id is its slot
// in that block (`rows::Slots`), so no two nodes can share an id. A page shows
// as many rows as fit in `uikit`'s caps on one window, then how many more.
pub const APP_ITEM: NodeId = 1000;
pub const ACCESS: NodeId = 2000;
pub const ACCESS_TEXT: NodeId = 3000;
pub const REVOKE: NodeId = 4000;
// Rules and profiles.
pub const RULES: NodeId = 30;
pub const NO_RULES: NodeId = 32;
pub const MORE_RULES: NodeId = 34;
pub const MORE_PROFILES: NodeId = 35;
pub const RULE: NodeId = 5000;
pub const RULE_TEXT: NodeId = 6000;
pub const REMOVE: NodeId = 7000;
pub const PROFILES: NodeId = 31;
pub const PROFILE_ITEM: NodeId = 8000;
// Keep running.
pub const NO_NAP: NodeId = 33;
pub const MORE_NAP: NodeId = 36;
pub const KEEP: NodeId = 9000;
// Updates.
pub const WINDOW_ON: NodeId = 40;
pub const START: NodeId = 41;
pub const END: NodeId = 42;
// About.
pub const ABOUT: NodeId = 10_000;

/// Most slots of one kind of row: one block of ids.
pub const MAX_ROWS: usize = 1000;
/// The window's size.
pub const SIZE: (u16, u16) = (640, 460);
/// The restart window a switch turns on with: 02:00 to 05:00.
pub const DEFAULT_WINDOW: (u16, u16) = (120, 300);

/// What the window shows that the model does not hold.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub page: usize,
    /// The app chosen on "What apps can do", by name: a place in the list would
    /// pass to another app when one above it goes.
    pub app: Option<String>,
    /// What the rows showed when the tree was last built, so an event names
    /// what the person saw, not what is at that place now.
    pub(crate) shown: Shown,
    /// The control the last change came from, put again on the next draw
    /// ([`redraw`]).
    pub(crate) touched: Option<NodeId>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Shown {
    pub(crate) apps: Vec<String>,
    pub(crate) grants: Slots<u32>,
    pub(crate) rules: Slots<RuleRef>,
    pub(crate) profiles: Vec<String>,
    pub(crate) keep: Slots<String>,
}

impl View {
    pub fn page(page: usize) -> View {
        View {
            page,
            ..View::default()
        }
    }
}

/// `hh:mm` for minute `m` of the day.
pub fn clock(m: u16) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// The window's tree for `settings` as `view` shows it.
pub fn build(settings: &Settings, view: &mut View) -> Vec<Entry> {
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
        Page::Rules => rules(settings, &mut view.shown, &mut tree),
        Page::Apps => apps(settings, &mut view.shown, &mut tree),
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

fn access(s: &Settings, view: &mut View, tree: &mut Vec<Entry>) {
    view.shown.apps.clear();
    if s.access.is_empty() {
        tree.push(entry(
            NO_ACCESS,
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
    tree.push(entry(
        APP_LIST,
        PAGE,
        wide(),
        Widget::ItemList {
            label: String::from("Apps"),
            selected: None,
        },
    ));
    let list = tree.len() - 1;
    let mut room = Room::of(tree);
    for app in s.access.iter().take(MAX_ROWS) {
        let item = entry(
            APP_ITEM + view.shown.apps.len() as u32,
            APP_LIST,
            wide(),
            label(&app.app, TypeStyle::Body, Tone::Normal),
        );
        if !room.take(core::slice::from_ref(&item)) {
            break;
        }
        view.shown.apps.push(app.app.clone());
        tree.push(item);
    }
    more(tree, MORE_APPS, s.access.len() - view.shown.apps.len());
    let selected = view
        .app
        .as_ref()
        .and_then(|name| view.shown.apps.iter().position(|n| n == name));
    if let Widget::ItemList { selected: at, .. } = &mut tree[list].widget {
        *at = selected.map(|i| i as u32);
    }
    let Some(app) = selected.map(|i| &s.access[i]) else {
        view.shown.grants = Slots::default();
        return;
    };
    let grants: Vec<u32> = app.access.iter().map(|x| x.grant).collect();
    let slots = view.shown.grants.place(&grants);
    let mut fitted = 0;
    for (x, slot) in app.access.iter().zip(slots) {
        let at = ACCESS + slot as u32;
        let row = [
            entry(at, PAGE, wide(), row()),
            entry(
                ACCESS_TEXT + slot as u32,
                at,
                wide(),
                label(
                    &format!("{} ({})", x.what, x.why),
                    TypeStyle::Body,
                    Tone::Normal,
                ),
            ),
            entry(
                REVOKE + slot as u32,
                at,
                Layout::FIT,
                Widget::Button {
                    text: String::from("Revoke"),
                    action: None,
                },
            ),
        ];
        if !room.take(&row) {
            break;
        }
        tree.extend(row);
        fitted += 1;
    }
    more(tree, MORE_ACCESS, app.access.len() - fitted);
}

/// "`n` more not shown" under the rows, if any did not fit.
fn more(tree: &mut Vec<Entry>, id: NodeId, n: usize) {
    if n > 0 {
        tree.push(entry(
            id,
            PAGE,
            wide(),
            label(&format!("{n} more not shown"), TypeStyle::Body, Tone::Muted),
        ));
    }
}

fn rules(s: &Settings, shown: &mut Shown, tree: &mut Vec<Entry>) {
    tree.push(entry(
        RULES,
        PAGE,
        wide(),
        label("Standing rules", TypeStyle::Label, Tone::Normal),
    ));
    if s.rules.is_empty() {
        tree.push(entry(
            NO_RULES,
            PAGE,
            wide(),
            label("None: every ask still asks.", TypeStyle::Body, Tone::Muted),
        ));
    }
    let ids: Vec<RuleRef> = s.rules.iter().map(|r| r.id).collect();
    let slots = shown.rules.place(&ids);
    let mut room = Room::of(tree);
    let mut fitted = 0;
    for (r, slot) in s.rules.iter().zip(slots) {
        let at = RULE + slot as u32;
        let row = [
            entry(at, PAGE, wide(), row()),
            entry(
                RULE_TEXT + slot as u32,
                at,
                wide(),
                label(&r.text, TypeStyle::Body, Tone::Normal),
            ),
            entry(
                REMOVE + slot as u32,
                at,
                Layout::FIT,
                Widget::Button {
                    text: String::from("Remove"),
                    action: Some(String::from("rules_remove")),
                },
            ),
        ];
        if !room.take(&row) {
            break;
        }
        tree.extend(row);
        fitted += 1;
    }
    more(tree, MORE_RULES, s.rules.len() - fitted);
    shown.profiles.clear();
    let list = entry(
        PROFILES,
        PAGE,
        wide(),
        Widget::ItemList {
            label: String::from("Profiles"),
            selected: None,
        },
    );
    if !room.take(core::slice::from_ref(&list)) {
        return;
    }
    tree.push(list);
    let at = tree.len() - 1;
    for name in s.profiles.iter().take(MAX_ROWS) {
        let item = entry(
            PROFILE_ITEM + shown.profiles.len() as u32,
            PROFILES,
            wide(),
            label(name, TypeStyle::Body, Tone::Normal),
        );
        if !room.take(core::slice::from_ref(&item)) {
            break;
        }
        shown.profiles.push(name.clone());
        tree.push(item);
    }
    let selected = s
        .profile
        .as_ref()
        .and_then(|p| shown.profiles.iter().position(|n| n == p));
    if let Widget::ItemList { selected: sel, .. } = &mut tree[at].widget {
        *sel = selected.map(|i| i as u32);
    }
    more(tree, MORE_PROFILES, s.profiles.len() - shown.profiles.len());
}

fn apps(s: &Settings, shown: &mut Shown, tree: &mut Vec<Entry>) {
    if s.nappable.is_empty() {
        tree.push(entry(
            NO_NAP,
            PAGE,
            wide(),
            label("No app naps yet.", TypeStyle::Body, Tone::Muted),
        ));
    }
    let slots = shown.keep.place(&s.nappable);
    let mut room = Room::of(tree);
    let mut fitted = 0;
    for (app, slot) in s.nappable.iter().zip(slots) {
        let toggle = entry(
            KEEP + slot as u32,
            PAGE,
            wide(),
            Widget::Toggle {
                label: format!("Keep {app} running"),
                on: s.apps.keeps(app),
            },
        );
        if !room.take(core::slice::from_ref(&toggle)) {
            break;
        }
        tree.push(toggle);
        fitted += 1;
    }
    more(tree, MORE_NAP, s.nappable.len() - fitted);
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
