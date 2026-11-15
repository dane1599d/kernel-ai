//! The tray's drawing (ADR 0005 §8), right to left: the UTC clock in two
//! lines, the memory in use, and the Kai chip, code's words alone. The chip's
//! dot is `ok` when ready, a `muted` ring while warming and a `muted` dash
//! with no model, never `ask` and never `dim` (the chip takes the `hover`
//! fill); a 6 px `ai_mark` badge marks words from Kai while the pane was
//! closed. Text that would not fit its slot at the theme's size (large text,
//! legible type) is drawn smaller to fit, never cut. The Kai corner is hit
//! only.

use crate::desk::BarSpot;
use crate::geom::Rect;
use crate::paint::{Painter, Scene, disc, fill, g};
use crate::tray::{self, Dot};
use gfx2d::Canvas;
use gfx2d::icons::Icon;
use look::TypeStyle;

const CHIP_RADIUS: i32 = 4;
/// The chip's dot: its centre from the chip's left, and its radius.
const DOT_X: i32 = 14;
const DOT_R: f32 = 4.0;
/// Where the chip's text starts, and its room from the chip's right.
const CHIP_TEXT: i32 = 28;
const CHIP_END: i32 = 8;
/// The memory's icon and text.
const MEM_ICON: i32 = 16;
const MEM_TEXT: i32 = 30;
/// The clock's room above and below its two lines.
const CLOCK_PAD: i32 = 4;

impl Painter {
    /// The tray's chip, memory and clock, inside `clip`.
    pub(crate) fn tray(&mut self, c: &mut Canvas<'_>, clip: Rect, scene: &Scene<'_>) {
        let l = scene.desk.layout();
        let t = l.tray;
        if t.area.intersect(&clip).is_none() {
            return;
        }
        self.chip(c, clip, t.chip, scene);
        if let Some(memory) = t.memory {
            self.memory(c, clip, memory, scene);
        }
        if let Some(utc_s) = scene.bar.utc_s {
            self.clock(c, clip, t.clock, utc_s);
        }
    }

    fn chip(&mut self, c: &mut Canvas<'_>, clip: Rect, chip: Rect, scene: &Scene<'_>) {
        let Some(clip) = chip.intersect(&clip) else {
            return;
        };
        let desk = scene.desk;
        let hovered = desk.bar_hovered() == Some(BarSpot::Chip);
        if hovered {
            self.hover_fill(c, chip, clip, CHIP_RADIUS);
        }
        let compact = desk.layout().tray.memory.is_none();
        let open = desk.pane().is_some();
        let (dot, text) = tray::chip(scene.panel, scene.bar, scene.now_ms, open, compact);
        let at = (chip.x + DOT_X, chip.y + chip.h / 2);
        let muted = self.palette.muted;
        match dot {
            Dot::Ready => disc(c, clip, at, DOT_R, self.palette.ok),
            Dot::Warming => {
                disc(c, clip, at, DOT_R, muted);
                let ground = match hovered && self.palette.hover != self.palette.shell {
                    true => self.palette.hover,
                    false => self.palette.shell,
                };
                disc(c, clip, at, DOT_R - 1.5, ground);
            }
            Dot::None => {
                let dash = Rect::new(at.0 - 4, at.1 - 1, 8, 2);
                fill(c, dash, clip, muted);
            }
        }
        let x = chip.x + CHIP_TEXT;
        let room = (chip.right() - CHIP_END - x) as f32;
        let ink = self.palette.text;
        let style = TypeStyle::Small;
        let scale = self.text_fit(style, &text, room, chip.h as f32);
        let y = crate::paint::baseline(chip.y, chip.h, self.size(style) * scale);
        self.text
            .draw(c, g(clip), x as f32, y, style, scale, ink, &text);
        if desk.unread() {
            let badge = (chip.right() - 6, chip.y + 6);
            disc(c, clip, badge, 3.0, self.palette.ai_mark);
        }
    }

    fn memory(&mut self, c: &mut Canvas<'_>, clip: Rect, memory: Rect, scene: &Scene<'_>) {
        let Some(clip) = memory.intersect(&clip) else {
            return;
        };
        let muted = self.palette.muted;
        let at = (memory.x + 6, memory.y + (memory.h - MEM_ICON) / 2);
        self.icons
            .draw(c, (Icon::Memory, MEM_ICON), at, muted, g(clip));
        let text = tray::memory(scene.bar);
        let style = TypeStyle::Small;
        let room = (memory.right() - MEM_TEXT - memory.x) as f32;
        let scale = self.text_fit(style, &text, room, memory.h as f32);
        let y = crate::paint::baseline(memory.y, memory.h, self.size(style) * scale);
        let x = (memory.x + MEM_TEXT) as f32;
        self.text.draw(c, g(clip), x, y, style, scale, muted, &text);
    }

    /// "14:05" over "14 Nov UTC", right-aligned in the slot, both lines drawn
    /// smaller together when they would not fit it.
    fn clock(&mut self, c: &mut Canvas<'_>, clip: Rect, slot: Rect, utc_s: u64) {
        let Some(clip) = slot.intersect(&clip) else {
            return;
        };
        let style = TypeStyle::MonoTitle;
        let time = crate::clock::time(utc_s);
        let date = crate::clock::date(utc_s);
        let line = self.text.style(style).line;
        let room_h = (slot.h - 2 * CLOCK_PAD) as f32 / 2.0;
        let scale = self
            .text_fit(style, &time, slot.w as f32, room_h)
            .min(self.text_fit(style, &date, slot.w as f32, room_h));
        let line_h = (line * scale) as i32;
        let top = slot.y + (slot.h - 2 * line_h) / 2;
        let size = self.size(style) * scale;
        let lines = [
            (time, top, self.palette.text),
            (date, top + line_h, self.palette.muted),
        ];
        for (text, at, ink) in lines {
            let w = self.text.measure(style, &text).0 * scale;
            let x = slot.right() as f32 - w;
            let y = crate::paint::baseline(at, line_h, size);
            self.text.draw(c, g(clip), x, y, style, scale, ink, &text);
        }
    }

    /// The scale (at most 1) that fits `text` in `style` into `w` across and
    /// one line of it into `h` down.
    fn text_fit(&self, style: TypeStyle, text: &str, w: f32, h: f32) -> f32 {
        let (tw, line) = self.text.measure(style, text);
        let across = if tw > w { w / tw } else { 1.0 };
        let down = if line > h { h / line } else { 1.0 };
        across.min(down).max(0.1)
    }

    /// The theme's size for `style`.
    fn size(&self, style: TypeStyle) -> f32 {
        self.text.style(style).size
    }

    /// The hover cue on `r`: the `hover` fill, or where `hover` is the
    /// taskbar's own ground (high contrast), a 1 px `focus` frame.
    pub(crate) fn hover_fill(&mut self, c: &mut Canvas<'_>, r: Rect, clip: Rect, radius: i32) {
        match self.palette.hover == self.palette.shell {
            true => crate::paint::frame(c, r, clip, self.palette.focus, 1),
            false => self.rounded(c, r, clip, self.palette.hover, radius),
        }
    }
}
