//! The tray's drawing (ADR 0005 §8), right to left: the UTC clock in two
//! lines, the memory in use, and the Kai chip, code's words alone. The chip's
//! dot is `ok` when ready, a `dim` ring while warming and a `dim` dash with
//! no model, never `ask`; a 6 px `ai` badge marks words from Kai while the
//! pane was closed. The Kai corner is hit only.

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
/// Where the chip's text starts.
const CHIP_TEXT: i32 = 28;
/// The memory's icon and text.
const MEM_ICON: i32 = 16;
const MEM_TEXT: i32 = 30;
/// The clock's two baselines, from the taskbar's top.
const TIME_BASE: i32 = 20;
const DATE_BASE: i32 = 36;

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
        if desk.bar_hovered() == Some(BarSpot::Chip) {
            self.rounded(c, chip, clip, self.palette.hover, CHIP_RADIUS);
        }
        let compact = desk.layout().tray.memory.is_none();
        let open = desk.pane().is_some();
        let (dot, text) = tray::chip(scene.panel, scene.bar, scene.now_ms, open, compact);
        let at = (chip.x + DOT_X, chip.y + chip.h / 2);
        match dot {
            Dot::Ready => disc(c, clip, at, DOT_R, self.palette.ok),
            Dot::Warming => {
                disc(c, clip, at, DOT_R, self.palette.dim);
                let ground = match desk.bar_hovered() == Some(BarSpot::Chip) {
                    true => self.palette.hover,
                    false => self.palette.shell,
                };
                disc(c, clip, at, DOT_R - 1.5, ground);
            }
            Dot::None => {
                let dash = Rect::new(at.0 - 4, at.1 - 1, 8, 2);
                fill(c, dash, clip, self.palette.dim);
            }
        }
        let y = self.baseline(chip.y, chip.h, TypeStyle::Small);
        let x = (chip.x + CHIP_TEXT) as f32;
        let ink = self.palette.text;
        self.draw_text(c, clip, x, y, TypeStyle::Small, ink, &text);
        if desk.unread() {
            disc(
                c,
                clip,
                (chip.right() - 6, chip.y + 6),
                3.0,
                self.palette.ai,
            );
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
        let y = self.baseline(memory.y, memory.h, TypeStyle::Small);
        let x = (memory.x + MEM_TEXT) as f32;
        let text = tray::memory(scene.bar);
        self.draw_text(c, clip, x, y, TypeStyle::Small, muted, &text);
    }

    /// "14:05" over "14 Nov UTC", right-aligned in the slot.
    fn clock(&mut self, c: &mut Canvas<'_>, clip: Rect, slot: Rect, utc_s: u64) {
        let Some(clip) = slot.intersect(&clip) else {
            return;
        };
        let style = TypeStyle::MonoTitle;
        let lines = [
            (crate::clock::time(utc_s), TIME_BASE, self.palette.text),
            (crate::clock::date(utc_s), DATE_BASE, self.palette.muted),
        ];
        for (text, base, ink) in lines {
            let w = self.text.measure(style, &text).0;
            let x = slot.right() as f32 - w;
            self.draw_text(c, clip, x, slot.y + base, style, ink, &text);
        }
    }
}
