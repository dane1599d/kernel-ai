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

    /// Asks `desktop` for its events before a window opens on a connection
    /// with nothing open there (`Desk::ask_before_open`). The greeting comes
    /// in the reply to any call, so the first window is drawn at the scale
    /// it prefers from its first picture; and a `desktop` that forgot the
    /// connection while nothing was shown greets it now, before the open,
    /// whose reply carried it and made comphost open every window a second
    /// time (H3-26).
    pub(super) fn greet_once(&mut self) {
        if !self.desk.ask_before_open() {
            return;
        }
        let Ok(len) = sys::call_route(self.route, &[], &mut self.reply, CALL_MS) else {
            return;
        };
        let reply = &self.reply[..len.min(self.reply.len())];
        let (told, anew) = self.desk.told_before_open(reply);
        if anew {
            println!("comphost: desktop had forgotten it; its windows open anew");
        }
        self.later.extend(told);
    }
}
