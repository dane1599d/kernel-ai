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
use look::TypeStyle;

/// An item's icon.
const ICON: i32 = 24;
/// A tooltip's padding, the gap between its label and title, its radius.
const TIP_PAD: i32 = 8;
const TIP_GAP: i32 = 6;
const TIP_RADIUS: i32 = 4;

/// `text` in Small cut to `room` px, ending in "…" when it was cut.
fn cut(text: &gfx2d::Text, s: &str, room: f32) -> alloc::string::String {
    let width = |t: &str| text.measure(TypeStyle::Small, t).0;
    if width(s) <= room {
        return alloc::string::String::from(s);
    }
    let mut out = alloc::string::String::new();
    for ch in s.chars() {
        out.push(ch);
        let mut tried = out.clone();
        tried.push('\u{2026}');
        if width(&tried) > room {
            out.pop();
            break;
        }
    }
    out.push('\u{2026}');
    out
}
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
        self.cover(bar, 0, clip);
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
        // A pin with no window has no pill.
        let (w, color) = match app.pill(desk) {
            Some(Pill::Focused) => (16, self.palette.accent),
            Some(Pill::Running) => (6, self.palette.muted),
            Some(Pill::Minimized) => (4, self.palette.muted),
            None => return,
        };
        self.pill(c, clip, r, w, color);
    }

    /// The tooltip over the hovered taskbar item, once it is due (ADR 0005
    /// §8): `raised`, radius 4, a 1 px `line_strong` frame; the kernel's
    /// label in mono `dim` first, then the app's title in `text`, at most
    /// 320 wide and cut at its end. The taskbar draws app text only here.
    pub(crate) fn tooltip(&mut self, c: &mut Canvas<'_>, clip: Rect, scene: &Scene<'_>) {
        let desk = scene.desk;
        let Some(i) = desk.tooltip() else {
            return;
        };
        let items = desk.taskbar_items();
        let (label, title) = match items.get(i) {
            Some(Item::Kai) => (
                alloc::string::String::new(),
                alloc::string::String::from("Kai"),
            ),
            Some(Item::More) => (
                alloc::string::String::new(),
                alloc::string::String::from("More apps"),
            ),
            Some(Item::App(app)) => {
                let title = app
                    .windows
                    .last()
                    .and_then(|id| desk.window(*id))
                    .map_or(alloc::string::String::new(), |w| w.title.clone());
                (app.label.clone(), title)
            }
            None => return,
        };
        let area = desk.tooltip_area(i, items.len());
        let Some(clip) = area.intersect(&clip) else {
            return;
        };
        let label_w = self.text.measure(TypeStyle::MonoTitle, &label).0 as i32;
        let title_w = self.text.measure(TypeStyle::Small, &title).0 as i32;
        let gap = if label.is_empty() || title.is_empty() {
            0
        } else {
            TIP_GAP
        };
        let w = (TIP_PAD * 2 + label_w + gap + title_w).min(area.w);
        let item = desk.item_rect(i, items.len());
        let x = (item.x + item.w / 2 - w / 2).clamp(area.x, area.right() - w);
        let bubble = Rect::new(x, area.y, w, area.h);
        self.rounded(c, bubble, clip, self.palette.line_strong, TIP_RADIUS);
        self.cover(bubble, TIP_RADIUS, clip);
        self.rounded(
            c,
            bubble.grow(-1),
            clip,
            self.palette.raised,
            TIP_RADIUS - 1,
        );
        let inside = bubble.grow(-1).intersect(&clip).unwrap_or_default();
        let mut tx = (bubble.x + TIP_PAD) as f32;
        if !label.is_empty() {
            let y = self.baseline(bubble.y, bubble.h, TypeStyle::MonoTitle);
            let dim = self.palette.dim;
            tx += self.draw_text(c, inside, tx, y, TypeStyle::MonoTitle, dim, &label) + gap as f32;
        }
        let text_end = bubble.right() - TIP_PAD;
        let room = (text_end as f32 - tx).max(0.0);
        let shown = cut(&self.text, &title, room);
        let y = self.baseline(bubble.y, bubble.h, TypeStyle::Small);
        let ink = self.palette.text;
        self.draw_text(c, inside, tx, y, TypeStyle::Small, ink, &shown);
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
