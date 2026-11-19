//! A send's text in the Kai pane while its card is up (ADR 0005 §7,
//! "Amended 2026-11-19"): in the history rectangle, in place of the
//! history, in the AI's look (the `Kai` label in `ai`, its 2 px rule, sans
//! `text`), since the text is the AI's words; never in the card and never in
//! the trusted look. Over it is a header code writes, mono `dim` as the
//! status row's: how many bytes go where, the digest the card shows, and
//! which lines show. Only a [`SendPreview`] reaches here, whose text
//! `desktop` has checked against the card's digest.

use crate::geom::Rect;
use crate::paint::{Painter, fill};
use crate::sendview::SendPreview;
use alloc::string::String;
use alloc::vec::Vec;
use gfx2d::Canvas;
use look::TypeStyle;

/// The AI's rule, and the space after it, as the history's.
const RULE: i32 = 2;
const INDENT: i32 = 12;

impl Painter {
    /// `preview`'s text as lines laid out in `area`, wrapped as the AI's
    /// words are; and how many of them fit under the header and the label.
    pub fn preview_layout(&self, area: Rect, preview: &SendPreview) -> (Vec<String>, usize) {
        let text = &self.text;
        let mut measure = |t: &str| text.measure(TypeStyle::Body, t).0;
        let width = (area.w - RULE - INDENT).max(1) as f32;
        let lines = crate::chat::wrap(&preview.shown(), width, &mut measure);
        let line_h = self.text.measure(TypeStyle::Body, "x").1 as i32;
        let head = self.text.measure(TypeStyle::Mono, "x").1 as i32;
        let label = self.text.measure(TypeStyle::Small, "Kai").1 as i32;
        let rows = ((area.h - head - label).max(0) / line_h.max(1)) as usize;
        (lines, rows)
    }

    /// `preview` in `area` of the canvas, nothing drawn outside it.
    pub(crate) fn send_preview(&mut self, c: &mut Canvas<'_>, area: Rect, preview: &SendPreview) {
        if area.is_empty() {
            return;
        }
        let (lines, rows) = self.preview_layout(area, preview);
        let header: String = preview.header(rows, lines.len());
        let head = self.text.measure(TypeStyle::Mono, "x").1 as i32;
        let mut y = area.y;
        let base = self.baseline(y, head, TypeStyle::Mono);
        let dim = self.palette.dim;
        self.draw_text(c, area, area.x as f32, base, TypeStyle::Mono, dim, &header);
        y += head;
        let label = self.text.measure(TypeStyle::Small, "Kai").1 as i32;
        let base = self.baseline(y, label, TypeStyle::Small);
        let ai = self.palette.ai;
        self.draw_text(c, area, area.x as f32, base, TypeStyle::Small, ai, "Kai");
        y += label;
        let line_h = self.text.measure(TypeStyle::Body, "x").1 as i32;
        let shown: Vec<&String> = lines.iter().skip(preview.scrolled).take(rows).collect();
        let h = line_h * shown.len() as i32;
        fill(c, Rect::new(area.x, y, RULE, h), area, self.palette.ai);
        let text = self.palette.text;
        for line in shown {
            let base = self.baseline(y, line_h, TypeStyle::Body);
            let x = (area.x + RULE + INDENT) as f32;
            self.draw_text(c, area, x, base, TypeStyle::Body, text, line);
            y += line_h;
        }
    }
}
