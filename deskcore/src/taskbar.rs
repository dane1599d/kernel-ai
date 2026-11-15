//! The taskbar's items (ADR 0005 §8): the Kai button, then the pins
//! (terminal, files, notes, editor, monitor, settings, each skipped if not
//! installed), then one item a running app that is not pinned, in the order
//! its first window opened. A pin and its windows share one item. Both are
//! matched by the kernel's label for the process, never by title, so an app
//! cannot pose as another here. Past the most items the screen allows the
//! last slot is "more", which opens All apps. The taskbar draws no app text
//! (its tooltips put the kernel's label first).

use crate::desk::{Desk, Focus, WindowId};
use alloc::string::String;
use alloc::vec::Vec;
use deskgeom::taskbar::max_items;

/// The image's own apps, every image's: `desktop` pins these until `pkg`
/// lists what is installed. The Editor and Models come only with a data
/// disk.
pub const IMAGE_APPS: [&str; 5] = ["notes", "files", "terminal", "settings", "monitor"];

/// The pinned apps, in order: each shows when it is installed.
pub const PINS: [&str; 6] = [
    "terminal", "files", "notes", "editor", "monitor", "settings",
];

/// The app a kernel label is: `notes-window` is notes, `term` the terminal.
pub fn app_of(label: &str) -> &str {
    match label.strip_suffix("-window").unwrap_or(label) {
        "term" => "terminal",
        app => app,
    }
}

/// One item on the taskbar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// The Kai button: it opens the Kai pane (the menu from PR 8).
    Kai,
    /// A pinned or running app.
    App(App),
    /// What did not fit: it opens All apps.
    More,
}

/// An app's item: the app (what `open <name>` names), the kernel label of
/// its windows (the pin's name while it has none), whether it is pinned, and
/// its windows, bottom to top as they stack, so the last is the most recent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    pub name: String,
    pub label: String,
    pub pinned: bool,
    pub windows: Vec<WindowId>,
}

/// How an app's running pill is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pill {
    /// One of its windows has the keys: 16 wide, `accent`.
    Focused,
    /// Running, none focused: 6 wide, `muted`.
    Running,
    /// Every window minimized: 4 wide, `muted`.
    Minimized,
}

impl App {
    /// How its pill is drawn; none for a pin with no window.
    pub fn pill(&self, desk: &Desk) -> Option<Pill> {
        if self.windows.is_empty() {
            return None;
        }
        Some(self.window_pill(desk))
    }

    fn window_pill(&self, desk: &Desk) -> Pill {
        let shown = |id: &WindowId| desk.window(*id).is_some_and(|w| !w.minimized);
        if self
            .windows
            .iter()
            .any(|id| desk.focus() == Focus::Window(*id) && shown(id))
        {
            return Pill::Focused;
        }
        match self.windows.iter().any(shown) {
            true => Pill::Running,
            false => Pill::Minimized,
        }
    }
}

/// The taskbar's items now: the Kai button, the installed pins, the other
/// running apps in open order, and "more" in the last slot when they do not
/// all fit.
pub fn items(desk: &Desk) -> Vec<Item> {
    let windows_of = |name: &str| -> Vec<WindowId> {
        desk.windows()
            .iter()
            .filter(|w| app_of(&w.label) == name)
            .map(|w| w.id)
            .collect()
    };
    let label_of = |windows: &[WindowId], name: &str| {
        windows
            .first()
            .and_then(|id| desk.window(*id))
            .map_or(String::from(name), |w| w.label.clone())
    };
    let mut apps: Vec<App> = PINS
        .iter()
        .filter(|pin| desk.installed().iter().any(|name| name == *pin))
        .map(|&pin| {
            let windows = windows_of(pin);
            App {
                name: String::from(pin),
                label: label_of(&windows, pin),
                pinned: true,
                windows,
            }
        })
        .collect();
    // The rest, in open order: by each app's first window, the lowest id.
    let mut running: Vec<(WindowId, &str)> = Vec::new();
    for w in desk.windows() {
        let name = app_of(&w.label);
        if apps.iter().any(|a| a.name == name) {
            continue;
        }
        match running.iter_mut().find(|(_, n)| *n == name) {
            Some((first, _)) => *first = (*first).min(w.id),
            None => running.push((w.id, name)),
        }
    }
    running.sort();
    for (_, name) in running {
        let windows = windows_of(name);
        apps.push(App {
            name: String::from(name),
            label: label_of(&windows, name),
            pinned: false,
            windows,
        });
    }
    let most = max_items(desk.layout().width);
    let fits = apps.len() < most;
    if !fits {
        apps.truncate(most - 2);
    }
    let mut items = alloc::vec![Item::Kai];
    items.extend(apps.into_iter().map(Item::App));
    if !fits {
        items.push(Item::More);
    }
    items
}
