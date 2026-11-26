//! The Kai pane's drawing (ADR 0005 §8, "The Kai pane"): `shell`, radius 8,
//! a 1 px `line_strong` frame and the flyout shadow, over the windows. Its
//! header has the Kai mark, "Kai" and the model's state (code's, from the
//! bar), and a close item; under it the history, the code-only status row
//! and the input. It is painted as one layer, so nothing in it reaches past
//! its rounded edge, and nothing of it is drawn while it is closed.

use crate::geom::Rect;
use crate::paint::{Painter, Scene, fill, g, opaque};
use crate::panel::State;
use deskgeom::pane::{self, PaneParts};
use gfx2d::Image;
use gfx2d::icons::Icon;
use look::TypeStyle;

/// The pane's corner radius (`look::metric::RADIUS_MD`).
const RADIUS: i32 = look::metric::RADIUS_MD as i32;
/// The input field's corner radius.
const INPUT_RADIUS: i32 = 6;
/// An item's glyph.
const GLYPH: i32 = 16;

/// The pane's header on the screen while it is open: the model's state there
/// changes with the bar. Empty while closed.
pub fn header_area(desk: &crate::desk::Desk) -> Rect {
    match desk.pane() {
        Some(_) => desk.layout().pane_parts().header,
        None => Rect::default(),
    }
}

impl Painter {
    /// The pane, if it is open, inside `clip`.
    pub(crate) fn pane(&mut self, c: &mut gfx2d::Canvas<'_>, clip: Rect, scene: &Scene<'_>) {
        let Some(at) = scene.desk.pane() else {
            return;
        };
        if crate::paint::reach(at).intersect(&clip).is_none() {
            return;
        }
        self.cast(c, at, clip);
        let layer = self.pane_layer(at, scene);
        c.draw_window(&layer, (at.x, at.y), g(clip), RADIUS, true);
        self.cover(at, RADIUS, clip);
    }

    /// The pane's pixels, in its own px.
    fn pane_layer(&mut self, at: Rect, scene: &Scene<'_>) -> Image {
        let local = Rect::new(0, 0, at.w, at.h);
        let parts = pane::parts(local);
        let mut layer = Image::new(local.w, local.h);
        let mut c = layer.canvas();
        fill(&mut c, local, local, self.palette.line_strong);
        fill(&mut c, local.grow(-1), local, self.palette.shell);
        self.pane_header(&mut c, &parts, scene);
        // A send's card up: its text in the history's place, never both.
        let preview = scene
            .preview
            .filter(|p| scene.ask.is_some_and(|card| card.id == p.ask));
        match preview {
            Some(preview) => self.send_preview(&mut c, parts.history, preview),
            None => self.chat(&mut c, parts.history, scene),
        }
        let rule = Rect::new(parts.status.x + 1, parts.status.y, parts.status.w - 2, 1);
        fill(&mut c, rule, local, self.palette.line);
        let thinking = matches!(scene.panel.state(), State::Thinking { .. });
        let status_end = match thinking {
            true => {
                self.item(&mut c, parts.stop, Icon::Stop, true);
                parts.stop.x - 8
            }
            false => crate::paint_panel::status_end(parts.status),
        };
        self.status(&mut c, parts.status, status_end, scene);
        self.pane_input(&mut c, &parts, scene, thinking);
        layer.round_corners(RADIUS);
        layer
    }

    /// The header: the Kai mark, "Kai", the model's state, the close item and
    /// the rule under it.
    fn pane_header(&mut self, c: &mut gfx2d::Canvas<'_>, parts: &PaneParts, scene: &Scene<'_>) {
        let header = parts.header;
        let mark = parts.mark;
        let ai = self.palette.ai;
        self.icons
            .draw(c, (Icon::Kai, mark.w), (mark.x, mark.y), ai, g(header));
        let y = self.baseline(header.y, header.h, TypeStyle::Label);
        let x = (header.x + pane::TITLE_X) as f32;
        let text = self.palette.text;
        let advance = self.draw_text(c, header, x, y, TypeStyle::Label, text, "Kai");
        let state = match (scene.bar.model, scene.panel.getting_ready()) {
            (false, _) => "no model",
            (true, true) => "getting ready",
            (true, false) => "ready",
        };
        let end = Rect::new(header.x, header.y, parts.close.x - 8 - header.x, header.h);
        let muted = self.palette.muted;
        let mut x = x + advance + 8.0;
        // The model's name, the shell's record of it, in the system's mono.
        if let Some(name) = scene.panel.model() {
            let y = self.baseline(header.y, header.h, TypeStyle::MonoTitle);
            x += self.draw_text(c, end, x, y, TypeStyle::MonoTitle, muted, name) + 8.0;
        }
        let y = self.baseline(header.y, header.h, TypeStyle::Small);
        self.draw_text(c, end, x, y, TypeStyle::Small, muted, state);
        self.item(c, parts.close, Icon::Close, false);
        let rule = Rect::new(header.x + 1, header.bottom(), header.w - 2, 1);
        fill(c, rule, header.grow(1), self.palette.line);
    }

    /// The input field, radius 6, `window` in a 1 px `line_strong` frame,
    /// with its send item (Stop while Kai thinks).
    fn pane_input(
        &mut self,
        c: &mut gfx2d::Canvas<'_>,
        parts: &PaneParts,
        scene: &Scene<'_>,
        thinking: bool,
    ) {
        let field = parts.input;
        let mut box_ = Image::new(field.w, field.h);
        {
            let mut b = box_.canvas();
            let all = Rect::new(0, 0, field.w, field.h);
            fill(&mut b, all, all, self.palette.line_strong);
            fill(&mut b, all.grow(-1), all, self.palette.window);
        }
        box_.round_corners(INPUT_RADIUS);
        c.draw(&box_, (field.x, field.y), g(field), true);
        self.input(c, field, parts.send.x - 4, scene);
        let icon = match thinking {
            true => Icon::Stop,
            false => Icon::Send,
        };
        self.item(c, parts.send, icon, false);
    }

    /// An item's glyph centred in `r`: `muted`, or `text` on a `hover` fill
    /// when `filled` (the status row's Stop).
    fn item(&mut self, c: &mut gfx2d::Canvas<'_>, r: Rect, icon: Icon, filled: bool) {
        let ink = match filled {
            true => {
                c.fill(g(r), opaque(self.palette.hover));
                self.palette.text
            }
            false => self.palette.muted,
        };
        let at = (r.x + (r.w - GLYPH) / 2, r.y + (r.h - GLYPH) / 2);
        self.icons.draw(c, (icon, GLYPH), at, ink, g(r));
    }
}
