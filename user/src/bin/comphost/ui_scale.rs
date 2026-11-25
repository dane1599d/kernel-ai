//! The windows at the desktop's scale (ADR 0005 section 9): what `Ui` does
//! when `desktop` says the scale it prefers, so `ui.rs` keeps to its size.

use super::*;

impl Ui {
    /// Every open window again, whole, from its last frame: a new theme or a
    /// new scale paints all of it, in buffers of the size now.
    pub(super) fn redraw_all(&mut self) {
        for (app, mut frame) in core::mem::take(&mut self.frames) {
            let (w, h) = frame.size;
            frame.damage = alloc::vec![uikit::geom::Rect::new(0, 0, w, h)];
            self.show(app, frame);
        }
    }

    /// Acts on a change of the desktop's scale among `told` (windows open, no
    /// new desktop): redraws them at it, and gives back the rest. It wakes no
    /// app, since the frames are kept here.
    pub(super) fn scaled(&mut self, told: Vec<Told>) -> Vec<Told> {
        if told.contains(&Told::Scale) {
            self.redraw_all();
            println!("comphost: drawing at scale {}/120", self.desk.scale());
        }
        told.into_iter().filter(|t| *t != Told::Scale).collect()
    }

    /// Asks `desktop` for its greeting before the first window opens, so the
    /// window is drawn at the scale it prefers from its first picture: the
    /// events (the theme, the scale) come in the reply to any call.
    pub(super) fn greet_once(&mut self) {
        if self.desk.greeted() {
            return;
        }
        let Ok(len) = sys::call_route(self.route, &[], &mut self.reply, CALL_MS) else {
            return;
        };
        let told = self.desk.told(&self.reply[..len.min(self.reply.len())]);
        self.later.extend(told);
    }
}
