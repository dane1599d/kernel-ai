//! The taskbar and tray on the desk (ADR 0005 §8): what a point on them
//! hits, what a click does, the hover, and the chip's unread badge. They are
//! drawn over the windows and hit before them, after the pane.
//!
//! An app's item: a click on it while one of its windows has the keys
//! minimizes that window; otherwise its most recent window comes back from
//! minimized, rises and takes the keys. The Kai button and the Kai corner
//! open the pane and focus its input; the chip toggles the pane; "more"
//! opens the Apps sheet.

use super::{Desk, Effect, Focus, Hit};
use crate::geom::{Point, Rect};
use crate::taskbar::{self, Item};
use alloc::vec::Vec;
use deskgeom::taskbar::item_hit;

/// A part of the tray.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayPart {
    Chip,
    Memory,
    Clock,
    /// The screen's bottom-right corner.
    Corner,
}

/// What on the taskbar is drawn with its hover fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarSpot {
    Item(usize),
    Chip,
}

impl Desk {
    /// The taskbar's items now ([`taskbar::items`]).
    pub fn taskbar_items(&self) -> Vec<Item> {
        taskbar::items(self)
    }

    /// Item `i` of the `n` now, as drawn: 40x40.
    pub fn item_rect(&self, i: usize, n: usize) -> Rect {
        let l = self.layout();
        deskgeom::taskbar::item(l.width, l.height, i as i32, n as i32)
    }

    /// The taskbar's spot under the pointer, drawn with its hover fill.
    pub fn bar_hovered(&self) -> Option<BarSpot> {
        self.bar_hover
    }

    /// Whether Kai said something while the pane was closed: the chip's
    /// badge, until the pane opens.
    pub fn unread(&self) -> bool {
        self.unread
    }

    /// Kai said something (`panel said`): unread while the pane is closed.
    pub fn kai_said(&mut self) {
        if self.pane_open || self.unread {
            return;
        }
        self.unread = true;
        let chip = self.layout().tray.chip;
        self.damage.add(chip);
    }

    /// What `p` hits on the taskbar or tray, if it is on the taskbar.
    pub(super) fn bar_hit(&self, p: Point) -> Option<Hit> {
        let l = self.layout();
        if !l.taskbar.contains(p) {
            return None;
        }
        let t = l.tray;
        let parts = [
            (Some(t.corner), TrayPart::Corner),
            (Some(t.chip), TrayPart::Chip),
            (t.memory, TrayPart::Memory),
            (Some(t.clock), TrayPart::Clock),
        ];
        for (r, part) in parts {
            if r.is_some_and(|r| r.contains(p)) {
                return Some(Hit::Tray(part));
            }
        }
        let n = self.taskbar_items().len() as i32;
        let item = (0..n).find(|&i| item_hit(l.width, l.height, i, n).contains(p));
        Some(Hit::Taskbar(item.map(|i| i as usize)))
    }

    /// A left click on the taskbar's item `i` or the tray's `part`.
    pub(super) fn bar_click(&mut self, hit: Hit, effects: &mut Vec<Effect>) {
        match hit {
            Hit::Taskbar(Some(i)) => match self.taskbar_items().into_iter().nth(i) {
                Some(Item::Kai) => self.set_focus(Focus::Panel, effects),
                Some(Item::App(app)) => self.app_click(&app.windows, effects),
                Some(Item::More) => effects.push(Effect::ShowApps),
                None => {}
            },
            Hit::Tray(TrayPart::Corner) => self.set_focus(Focus::Panel, effects),
            Hit::Tray(TrayPart::Chip) => match self.pane_open {
                true => self.close_pane(effects),
                false => self.set_focus(Focus::Panel, effects),
            },
            _ => {}
        }
    }

    /// An app's item clicked: its focused window minimizes; else its most
    /// recent window comes back, rises and takes the keys.
    fn app_click(&mut self, windows: &[super::WindowId], effects: &mut Vec<Effect>) {
        let focused = windows.iter().copied().find(|id| {
            self.focus == Focus::Window(*id) && self.window(*id).is_some_and(|w| !w.minimized)
        });
        if let Some(id) = focused {
            self.minimize(id, effects);
            return;
        }
        if let Some(&id) = windows.last() {
            self.activate(id, effects);
        }
    }

    /// The taskbar's hover for `hit`: damaged when it moves.
    pub(super) fn bar_hover_to(&mut self, hit: Option<Hit>) {
        let spot = match hit {
            Some(Hit::Taskbar(Some(i))) => Some(BarSpot::Item(i)),
            Some(Hit::Tray(TrayPart::Chip)) => Some(BarSpot::Chip),
            _ => None,
        };
        if spot != self.bar_hover {
            self.bar_hover = spot;
            self.bar_changed();
        }
    }

    /// What the taskbar shows changed: a window came or went, rose, was
    /// renamed, minimized or restored, the focus moved, or the hover.
    pub(super) fn bar_changed(&mut self) {
        let bar = self.layout().taskbar;
        self.damage.add(bar);
    }
}
