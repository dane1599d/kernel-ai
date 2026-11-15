//! The taskbar's drawing (ADR 0005 §8): `shell` with a 1 px `line` rule on
//! top; the Kai button (the Kai mark in `ai`, `ai_mark` while hovered, a
//! 16 px `ai_mark` pill while Kai thinks or acts, and a 2 px `ask` ring while
//! a card is up: a second trusted cue, as no app can paint the taskbar);
//! then each running app's icon with its pill; then "more". Hover fills are
//! `hover`, radius 4. No app text is drawn here.

use crate::desk::{BarSpot, Desk};
use crate::geom::Rect;
use crate::paint::{Painter, Scene, fill, g, opaque};
use crate::panel::State;
use crate::taskbar::{App, Item, Pill};
use gfx2d::icons::Icon;
use gfx2d::{Canvas, Image};

/// An item's icon.
const ICON: i32 = 24;
/// The hover fill's and the Kai button's ring's radii.
const HOVER_RADIUS: i32 = 4;
const RING_RADIUS: i32 = 8;
const RING: i32 = 2;
/// A running pill: 3 tall, 2 above the item's bottom.
const PILL_H: i32 = 3;
const PILL_UP: i32 = 2;

impl Painter {
    /// The taskbar and its tray, inside `clip`.
    pub(crate) fn taskbar(&mut self, c: &mut Canvas<'_>, clip: Rect, scene: &Scene<'_>) {
        let desk = scene.desk;
        let bar = desk.layout().taskbar;
        let Some(clip) = bar.intersect(&clip) else {
            return;
        };
        fill(c, bar, clip, self.palette.shell);
        fill(
            c,
            Rect::new(bar.x, bar.y, bar.w, 1),
            clip,
            self.palette.line,
        );
        let items = desk.taskbar_items();
        let n = items.len();
        for (i, item) in items.iter().enumerate() {
            let r = desk.item_rect(i, n);
            if r.intersect(&clip).is_none() {
                continue;
            }
            let hovered = desk.bar_hovered() == Some(BarSpot::Item(i));
            if hovered {
                self.hover_fill(c, r, clip, HOVER_RADIUS);
            }
            match item {
                Item::Kai => self.kai_button(c, clip, r, hovered, scene),
                Item::App(app) => self.app_item(c, clip, r, app, desk),
                Item::More => self.glyph(c, clip, r, Icon::More, self.palette.muted),
            }
        }
        self.tray(c, clip, scene);
    }

    /// The Kai button.
    fn kai_button(
        &mut self,
        c: &mut Canvas<'_>,
        clip: Rect,
        r: Rect,
        hovered: bool,
        scene: &Scene<'_>,
    ) {
        let mark = match hovered {
            true => self.palette.ai_mark,
            false => self.palette.ai,
        };
        if scene.desk.ask().is_some() {
            let ground = match hovered && self.palette.hover != self.palette.shell {
                true => self.palette.hover,
                false => self.palette.shell,
            };
            self.rounded(c, r, clip, self.palette.ask, RING_RADIUS);
            let inside = r.grow(-RING);
            self.rounded(c, inside, clip, ground, RING_RADIUS - RING);
        }
        self.glyph(c, clip, r, Icon::Kai, mark);
        // After the ring: while a card is up and Kai acts, both cues show.
        if matches!(
            scene.panel.state(),
            State::Thinking { .. } | State::Acting { .. }
        ) {
            self.pill(c, clip, r, 16, self.palette.ai_mark);
        }
    }

    /// A running app's item: its icon (or monogram) and its pill.
    fn app_item(&mut self, c: &mut Canvas<'_>, clip: Rect, r: Rect, app: &App, desk: &Desk) {
        match crate::paint::app_icon(&app.label) {
            Some(icon) => self.glyph(c, clip, r, icon, self.palette.text),
            None => {
                let side = crate::monogram::TILE;
                let at = (r.x + (r.w - side) / 2, r.y + (r.h - side) / 2);
                self.monogram(c, clip, at, &app.label);
            }
        }
        let (w, color) = match app.pill(desk) {
            Pill::Focused => (16, self.palette.accent),
            Pill::Running => (6, self.palette.muted),
            Pill::Minimized => (4, self.palette.muted),
        };
        self.pill(c, clip, r, w, color);
    }

    /// `icon` at 24 px, centred in `r`.
    fn glyph(&mut self, c: &mut Canvas<'_>, clip: Rect, r: Rect, icon: Icon, rgb: u32) {
        let at = (r.x + (r.w - ICON) / 2, r.y + (r.h - ICON) / 2);
        self.icons.draw(c, (icon, ICON), at, rgb, g(clip));
    }

    /// A pill `w` wide, centred under `r`'s middle, 2 above its bottom.
    fn pill(&mut self, c: &mut Canvas<'_>, clip: Rect, r: Rect, w: i32, rgb: u32) {
        let pill = Rect::new(
            r.x + (r.w - w) / 2,
            r.bottom() - PILL_UP - PILL_H,
            w,
            PILL_H,
        );
        self.rounded(c, pill, clip, rgb, 1);
    }

    /// `r` filled with `rgb`, its corners rounded to `radius`.
    pub(crate) fn rounded(
        &mut self,
        c: &mut Canvas<'_>,
        r: Rect,
        clip: Rect,
        rgb: u32,
        radius: i32,
    ) {
        if r.w <= 0 || r.h <= 0 {
            return;
        }
        let mut tile = Image::new(r.w, r.h);
        {
            let mut t = tile.canvas();
            t.fill(gfx2d::Rect::new(0, 0, r.w, r.h), opaque(rgb));
        }
        tile.round_corners(radius);
        c.draw(&tile, (r.x, r.y), g(clip), true);
    }
}
