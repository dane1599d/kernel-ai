//! The Kai pane on the desk (ADR 0005 §8): closed at boot, opened by
//! Ctrl+Space or a Super tap (each focuses its input), closed by its close
//! item. It floats over the windows; at 1280 wide and up it stays open when
//! the focus leaves it and maximized windows fit beside it, under 1280 it is
//! a flyout that closes when the focus leaves it.

use super::{Desk, Effect, Focus};
use crate::geom::{Point, Rect, Size};
use alloc::vec::Vec;
use deskgeom::PANE_BESIDE;

/// What a point on the open pane hits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanePart {
    /// The header's close item.
    Close,
    /// The header, the history and the pane's frame: a click focuses the
    /// input.
    Body,
    /// The status row: its undo and show links, where they are drawn.
    Status,
    /// The input field.
    Input,
    /// The send item, Stop while Kai thinks.
    Send,
}

impl Desk {
    /// The pane's rectangle while it is open.
    pub fn pane(&self) -> Option<Rect> {
        self.pane_open.then(|| self.layout().pane)
    }

    /// The pane's rectangle while it is open, else an empty one: what a
    /// change to the panel's input, state or history damages.
    pub fn pane_area(&self) -> Rect {
        self.pane().unwrap_or_default()
    }

    /// What `p` hits on the pane, if it is open and `p` is on it.
    pub(super) fn pane_hit(&self, p: Point) -> Option<PanePart> {
        let pane = self.pane()?;
        if !pane.contains(p) {
            return None;
        }
        let parts = self.layout().pane_parts();
        let part = if parts.close.contains(p) {
            PanePart::Close
        } else if parts.send.contains(p) {
            PanePart::Send
        } else if parts.input.contains(p) {
            PanePart::Input
        } else if parts.status.contains(p) {
            PanePart::Status
        } else {
            PanePart::Body
        };
        Some(part)
    }

    /// Opens or closes the pane: it and what a re-fit moves are damaged, and
    /// maximized windows are re-fitted ([`Desk::refit`]).
    pub(super) fn show_pane(&mut self, open: bool, effects: &mut Vec<Effect>) {
        if self.pane_open == open {
            return;
        }
        let pane = self.layout().pane;
        let before = self.layout().maximized;
        self.damage.add(pane.grow(crate::paint::SHADOW_REACH));
        self.pane_open = open;
        if self.layout().maximized != before {
            self.refit(effects);
        }
    }

    /// The pane's close item: the pane goes, and its input's focus with it.
    pub(super) fn close_pane(&mut self, effects: &mut Vec<Effect>) {
        if self.focus == Focus::Panel {
            self.leave_panel(effects);
        }
        self.show_pane(false, effects);
    }

    /// Esc in the input with nothing of the panel's running: the focus goes
    /// back to the window it came from, else the topmost; the pane stays
    /// open (a flyout closes as the focus leaves it).
    pub fn leave_pane(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        if self.focus == Focus::Panel {
            self.leave_panel(&mut effects);
        }
        effects
    }

    fn leave_panel(&mut self, effects: &mut Vec<Effect>) {
        let back = match self.before_panel {
            Focus::Window(id) if self.window(id).is_some_and(|w| !w.minimized) => Focus::Window(id),
            _ => self.top_visible().map_or(Focus::Nothing, Focus::Window),
        };
        self.set_focus(back, effects);
    }

    /// Focus moved from `from` to `to`: the input's look changes, the pane
    /// opens for its input, and a flyout closes when the focus leaves it.
    pub(super) fn pane_focus(&mut self, from: Focus, to: Focus, effects: &mut Vec<Effect>) {
        if to == Focus::Panel {
            // Back from a card, the focus came from the panel itself: Esc
            // still goes where it was before the panel took it.
            if !matches!(from, Focus::Ask(_)) {
                self.before_panel = from;
            }
            self.show_pane(true, effects);
        }
        if from == Focus::Panel && to != Focus::Panel && self.width < PANE_BESIDE {
            self.show_pane(false, effects);
        }
        if let Some(pane) = self.pane()
            && (from == Focus::Panel || to == Focus::Panel)
        {
            self.damage.add(pane);
        }
    }

    /// The maximize area moved: maximized windows fill it now, beside the
    /// pane while it is open at 1280 wide and up, else the work area. Each
    /// is moved now and told its new size, as a maximize does, whatever it
    /// last committed: a slow app's commit is not waited for.
    pub(super) fn refit(&mut self, effects: &mut Vec<Effect>) {
        let area = self.layout().maximized;
        let focus = self.focus;
        for w in self.windows.iter_mut().filter(|w| w.maximized()) {
            let content = crate::chrome::content_of(area);
            let old = w.frame;
            w.frame = Rect::new(area.x, area.y, old.w, old.h);
            self.damage.add(old.grow(crate::paint::SHADOW_REACH));
            self.damage.add(w.frame.grow(crate::paint::SHADOW_REACH));
            effects.push(Effect::Configure {
                window: w.id,
                size: Size {
                    w: content.w,
                    h: content.h,
                },
                maximized: true,
                activated: focus == Focus::Window(w.id),
            });
        }
    }
}
