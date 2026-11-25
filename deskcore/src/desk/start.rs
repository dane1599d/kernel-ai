//! Start and the taskbar's tooltip on the desk (ADR 0005 §8). Start opens
//! by a Super tap, Ctrl+Esc or the Kai button; it takes the keys (the
//! panel's focus, Start's host: `desktop` gives them to Start while it is
//! open) and is hit before everything but the card. A click outside it
//! closes it and goes nowhere else; Esc closes it; a card closes it, and so
//! does the focus leaving it. A tooltip shows over a taskbar item after
//! 500 ms of hover, label first, and repaints only as it comes and goes.

use super::{Desk, Effect, Focus, Hit, Part};
use crate::geom::Rect;
use alloc::vec::Vec;

/// How long the pointer rests on a taskbar item before its tooltip shows.
pub const TIP_MS: u64 = 500;

impl Desk {
    /// Start's rectangle while it is open.
    pub fn start(&self) -> Option<Rect> {
        self.start_open
            .then(|| deskgeom::start::start(self.width, self.height))
    }

    /// The wheel turned by `detents` (Kai Desk PR 9): Start's lists while
    /// the pointer is on Start, else the pane's history while it is on that.
    /// Over a window's content it goes to that window's client as a ring
    /// event (`Effect::Wheel`), focused or not, and moves no focus.
    pub(super) fn wheel(&mut self, detents: i32, effects: &mut Vec<Effect>) {
        if detents == 0 {
            return;
        }
        let p = self.pointer;
        if self.start().is_some_and(|r| r.contains(p)) {
            effects.push(Effect::StartWheel(detents));
            return;
        }
        let history = self.layout().pane_parts().history;
        if self.pane().is_some() && history.contains(p) {
            effects.push(Effect::PaneWheel(detents));
            return;
        }
        if let Hit::Window(window, Part::Content(at)) = self.hit(p) {
            effects.push(Effect::Wheel {
                window,
                at,
                detents,
            });
        }
    }

    /// Start's rectangle, open or not: what a change to it damages.
    pub fn start_area(&self) -> Rect {
        deskgeom::start::start(self.width, self.height)
    }

    /// Opens Start with its box focused; the pane stays as it is.
    pub fn open_start(&mut self, effects: &mut Vec<Effect>) {
        if self.start_open || self.ask.is_some() {
            return;
        }
        // Where the keys go back to when Start closes without a pick: the
        // window, or Kai's input, that had them.
        self.start_from = self.focus;
        if !matches!(self.focus, Focus::Panel | Focus::Ask(_)) {
            self.before_panel = self.focus;
        }
        self.tip_reset();
        self.start_open = true;
        let area = deskgeom::start::start(self.width, self.height);
        self.damage.add(area.grow(crate::paint::SHADOW_REACH));
        self.bar_changed();
        effects.push(Effect::Start(true));
        self.set_focus(Focus::Panel, effects);
    }

    /// Closes Start, if it is open.
    pub fn close_start(&mut self, effects: &mut Vec<Effect>) {
        if !self.start_open {
            return;
        }
        self.start_open = false;
        let area = deskgeom::start::start(self.width, self.height);
        self.damage.add(area.grow(crate::paint::SHADOW_REACH));
        self.bar_changed();
        effects.push(Effect::Start(false));
        // Start had the keys: they go back where they were when it opened,
        // never left in Kai's input unless they came from it.
        if self.focus == Focus::Panel {
            match self.start_from {
                Focus::Panel if self.pane_open => {}
                Focus::Window(id) if self.window(id).is_some_and(|w| !w.minimized) => {
                    self.set_focus(Focus::Window(id), effects);
                }
                Focus::Nothing => self.set_focus(Focus::Nothing, effects),
                _ => {
                    let next = self.top_visible().map_or(Focus::Nothing, Focus::Window);
                    self.set_focus(next, effects);
                }
            }
        }
    }

    /// Test only: Start drawn under a card, as no input can leave them, to
    /// show the card paints over it.
    #[cfg(test)]
    pub(crate) fn open_start_under_card(&mut self) {
        self.start_open = true;
    }

    /// The Kai button's and Super's toggle.
    pub(super) fn toggle_start(&mut self, effects: &mut Vec<Effect>) {
        match self.start_open {
            true => self.close_start(effects),
            false => self.open_start(effects),
        }
    }

    /// Ctrl+Space: the pane's input takes the keys, Start closing first.
    pub fn focus_pane(&mut self, effects: &mut Vec<Effect>) {
        self.close_start(effects);
        match self.focus == Focus::Panel {
            true => self.show_pane(true, effects),
            false => self.set_focus(Focus::Panel, effects),
        }
    }

    /// The taskbar item whose tooltip shows now, if any.
    pub fn tooltip(&self) -> Option<usize> {
        match (self.tip_shown, self.bar_hover) {
            (true, Some(super::BarSpot::Item(i))) => Some(i),
            _ => None,
        }
    }

    /// The tooltip box over item `i` of the `n` on the taskbar.
    pub fn tooltip_area(&self, i: usize, n: usize) -> Rect {
        deskgeom::start::tooltip(self.width, self.height, self.item_rect(i, n))
    }

    /// The clock moved on to `now_ms`: a tooltip due shows, once.
    pub fn tick(&mut self, now_ms: u64) {
        self.now_ms = self.now_ms.max(now_ms);
        let due = now_ms.saturating_sub(self.hover_since) >= TIP_MS;
        if let Some(super::BarSpot::Item(i)) = self.bar_hover
            && !self.tip_shown
            && due
            && self.ask.is_none()
            && !self.start_open
        {
            self.tip_shown = true;
            let n = self.taskbar_items().len();
            let tip = self.tooltip_area(i, n);
            self.damage.add(tip);
        }
    }

    /// The hover moved: a tooltip shown goes, and the next waits again.
    pub(super) fn tip_reset(&mut self) {
        if self.tip_shown {
            self.tip_shown = false;
            self.tip_repaint();
        }
        self.hover_since = self.now_ms;
    }

    /// The band a tooltip can take, damaged: it came, went or changed.
    pub(super) fn tip_repaint(&mut self) {
        let r = deskgeom::start::tooltip(self.width, self.height, Rect::new(0, 0, 0, 0));
        self.damage.add(Rect::new(0, r.y, self.width, r.h));
    }

    /// The pointer moved: Start's hovered spot (by geometry alone) repaints
    /// Start when it changes.
    pub(super) fn start_hover(&mut self) {
        let Some(area) = self.start() else {
            return;
        };
        let p = self.pointer;
        let spot = |n: i32, rect: &dyn Fn(i32) -> Rect| {
            (0..n).find(|&i| rect(i).contains(p)).unwrap_or(-1)
        };
        use deskgeom::start as geo;
        let now = [
            spot(geo::GRID_COLS * geo::GRID_ROWS, &|i| geo::cell(area, i)),
            spot(geo::RECENT_ROWS, &|i| geo::recent_row(area, i)),
            spot(geo::SEARCH_ROWS, &|i| geo::search_row(area, i)),
            spot(geo::ALL_ROWS, &|i| geo::all_row(area, i)),
        ];
        if now != self.start_spot {
            self.start_spot = now;
            self.damage.add(area);
        }
    }
}
