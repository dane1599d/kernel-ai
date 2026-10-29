//! Moving windows about: resizing through the client, Alt+Tab, raising,
//! minimizing and maximizing.

use super::{Desk, Effect, Focus, WindowId};
use crate::chrome::{self, Edges};
use crate::geom::{Point, Rect, Size};
use alloc::vec::Vec;

impl Desk {
    /// Asks the client for the size dragging its `edges` by `delta` gives.
    pub(super) fn resize(
        &mut self,
        id: WindowId,
        edges: Edges,
        start: Rect,
        delta: Point,
        effects: &mut Vec<Effect>,
    ) {
        let Some(w) = self.window(id) else {
            return;
        };
        let content = chrome::content_of(start);
        let dw = if edges.right {
            delta.x
        } else if edges.left {
            -delta.x
        } else {
            0
        };
        let dh = if edges.bottom {
            delta.y
        } else if edges.top {
            -delta.y
        } else {
            0
        };
        let size = Size {
            w: (content.w + dw).clamp(w.min.w, w.max.w),
            h: (content.h + dh).clamp(w.min.h, w.max.h),
        };
        // Left and top edges move the corner as well as the size.
        let x = if edges.left {
            content.right() - size.w
        } else {
            content.x
        };
        let y = if edges.top {
            content.bottom() - size.h
        } else {
            content.y
        };
        let frame = chrome::frame_for(Rect::new(x, y, content.w, content.h));
        let activated = self.focus == Focus::Window(id);
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            // Moved now; the size changes when the client commits it.
            if w.frame.x != frame.x || w.frame.y != frame.y {
                let old = w.frame;
                w.frame = Rect::new(frame.x, frame.y, old.w, old.h);
                let new = w.frame;
                self.damage.add(old);
                self.damage.add(new);
            }
            w.restore = None;
        }
        effects.push(Effect::Configure {
            window: id,
            size,
            maximized: false,
            activated,
        });
    }

    /// Alt+Tab: the next window down the stack comes to the top, minimized ones
    /// included, which come back.
    pub(super) fn cycle(&mut self, effects: &mut Vec<Effect>) {
        if self.windows.is_empty() {
            return;
        }
        let current = match self.focus {
            Focus::Window(id) => self.windows.iter().position(|w| w.id == id),
            _ => None,
        };
        // The bottom window rises: repeated presses walk the whole stack.
        let pick = match current {
            Some(_) if self.windows.len() > 1 => 0,
            Some(at) => at,
            None => self.windows.len() - 1,
        };
        let id = self.windows[pick].id;
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.minimized = false;
            let frame = w.frame;
            self.damage.add(frame);
        }
        self.raise(id);
        self.set_focus(Focus::Window(id), effects);
    }

    /// The agent's `ui_focus` (ADR 0005 §3: a row the agent runs may move
    /// focus): `id` comes back if it was minimized, rises and takes the keys.
    pub fn activate(&mut self, id: WindowId, effects: &mut Vec<Effect>) {
        let Some(w) = self.windows.iter_mut().find(|w| w.id == id) else {
            return;
        };
        w.minimized = false;
        let frame = w.frame;
        self.damage.add(frame);
        self.raise(id);
        self.set_focus(Focus::Window(id), effects);
    }

    pub(super) fn raise(&mut self, id: WindowId) {
        let Some(at) = self.windows.iter().position(|w| w.id == id) else {
            return;
        };
        if at + 1 == self.windows.len() {
            return;
        }
        let window = self.windows.remove(at);
        self.damage.add(window.frame);
        self.windows.push(window);
    }

    pub(super) fn minimize(&mut self, id: WindowId, effects: &mut Vec<Effect>) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.minimized = true;
            let frame = w.frame;
            self.damage.add(frame);
        }
        if self.focus == Focus::Window(id) {
            let next = self.top_visible().map_or(Focus::Nothing, Focus::Window);
            self.set_focus(next, effects);
        }
    }

    pub(super) fn toggle_maximize(&mut self, id: WindowId, effects: &mut Vec<Effect>) {
        let area = self.work_area();
        let activated = self.focus == Focus::Window(id);
        let Some(w) = self.windows.iter_mut().find(|w| w.id == id) else {
            return;
        };
        let (frame, maximized) = match w.restore.take() {
            Some(back) => (back, false),
            None => {
                w.restore = Some(w.frame);
                (area, true)
            }
        };
        // Moved now; the client resizes to the content and commits.
        let content = chrome::content_of(frame);
        let old = w.frame;
        let size = Size {
            w: content.w,
            h: content.h,
        };
        w.frame = Rect::new(frame.x, frame.y, old.w, old.h);
        let new = w.frame;
        self.damage.add(old);
        self.damage.add(new);
        effects.push(Effect::Configure {
            window: id,
            size,
            maximized,
            activated,
        });
    }

    pub(super) fn top_visible(&self) -> Option<WindowId> {
        self.windows
            .iter()
            .rev()
            .find(|w| !w.minimized)
            .map(|w| w.id)
    }
}
