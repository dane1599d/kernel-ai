//! The taskbar's items (ADR 0005 §8): the Kai button, then one item a
//! running app in the order its first window opened, matched by the
//! kernel's label for the process and never by title, so an app cannot pose
//! as another here. Past the most items the screen allows the last slot is
//! "more", which opens the Apps sheet. The taskbar draws no app text.

use crate::desk::{Desk, Focus, WindowId};
use alloc::string::String;
use alloc::vec::Vec;
use deskgeom::taskbar::max_items;

/// One item on the taskbar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// The Kai button: it opens the Kai pane (the menu from PR 8).
    Kai,
    /// A running app, by its kernel label.
    App(App),
    /// What did not fit: it opens the Apps sheet.
    More,
}

/// A running app's item: its label and its windows, bottom to top as they
/// stack, so the last is the most recent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    pub label: String,
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
    pub fn pill(&self, desk: &Desk) -> Pill {
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

/// The taskbar's items now: the Kai button, the apps in open order, and
/// "more" in the last slot when they do not all fit.
pub fn items(desk: &Desk) -> Vec<Item> {
    // Open order: by each label's first window, whose id is the lowest.
    let mut labels: Vec<(WindowId, &str)> = Vec::new();
    for w in desk.windows() {
        match labels.iter_mut().find(|(_, l)| *l == w.label) {
            Some((first, _)) => *first = (*first).min(w.id),
            None => labels.push((w.id, &w.label)),
        }
    }
    labels.sort();
    let most = max_items(desk.layout().width);
    let fits = labels.len() < most;
    let shown = match fits {
        true => labels.len(),
        false => most - 2,
    };
    let mut items = alloc::vec![Item::Kai];
    for &(_, label) in &labels[..shown] {
        let windows = desk
            .windows()
            .iter()
            .filter(|w| w.label == label)
            .map(|w| w.id)
            .collect();
        items.push(Item::App(App {
            label: String::from(label),
            windows,
        }));
    }
    if !fits {
        items.push(Item::More);
    }
    items
}
