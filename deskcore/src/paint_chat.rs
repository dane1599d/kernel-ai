//! Drawing the panel's history ([`crate::chat`]): a column centred on the
//! desk's background between the top bar and the panel, newest entry at the
//! bottom, drawn before the windows so each window covers it. Each kind has
//! its own look, so the AI's words never pass for the system's:
//!
//! - the person's line: `you` in `dim`, the line in mono `muted`;
//! - the AI's: `Kai` in `ai`, a 2 px `ai` rule down its left, its words in
//!   sans `text`. The system draws its own words in mono (the bar, cards,
//!   notes), never in this look;
//! - a note, code's result line or refusal: mono `dim`, as the bar showed it.

use crate::chat::{self, Entry, Who};
use crate::desk::Desk;
use crate::geom::Rect;
use crate::paint::{Painter, Scene, fill};
use alloc::string::String;
use alloc::vec::Vec;
use gfx2d::Canvas;
use look::TypeStyle;

/// The column's widest.
const MAX_W: i32 = 760;
/// Space between the column and the bars, and at its sides.
const MARGIN: i32 = 16;
/// Space between entries.
const GAP: i32 = 10;
/// The AI's rule, and the space after it.
const RULE: i32 = 2;
const INDENT: i32 = 12;

/// Where the history is drawn: what a change to it damages.
pub fn chat_area(desk: &Desk) -> Rect {
    let width = desk.top_bar().w;
    let top = desk.top_bar().bottom() + MARGIN;
    let bottom = desk.panel().y - MARGIN;
    let w = (width - 2 * MARGIN).clamp(0, MAX_W);
    Rect::new((width - w) / 2, top, w, (bottom - top).max(0))
}

/// One entry, laid out: its label, its lines and their look.
struct Laid {
    who: Who,
    label: &'static str,
    style: TypeStyle,
    lines: Vec<String>,
}

impl Laid {
    fn height(&self, label_h: i32, line_h: i32) -> i32 {
        let label = if self.label.is_empty() { 0 } else { label_h };
        label + line_h * self.lines.len() as i32
    }
}

impl Painter {
    pub(crate) fn chat(&mut self, c: &mut Canvas<'_>, clip: Rect, scene: &Scene<'_>) {
        if scene.chat.is_empty() {
            return;
        }
        let area = chat_area(scene.desk);
        let Some(clip) = area.intersect(&clip) else {
            return;
        };
        let label_h = self.text.measure(TypeStyle::Small, "Kai").1 as i32;
        let mut bottom = area.bottom();
        for entry in scene.chat.entries().rev() {
            let laid = self.lay(entry, area.w);
            let line_h = self.text.measure(laid.style, "x").1 as i32;
            let top = bottom - laid.height(label_h, line_h);
            self.entry(c, clip, area.x, top, &laid, (label_h, line_h));
            bottom = top - GAP;
            if bottom <= area.y {
                break;
            }
        }
    }

    /// `entry`'s lines at `width`, in its kind's look.
    fn lay(&self, entry: &Entry, width: i32) -> Laid {
        let (label, style, indent) = match entry.who {
            Who::You => ("you", TypeStyle::Mono, 0),
            Who::Kai => ("Kai", TypeStyle::Body, RULE + INDENT),
            Who::Note => ("", TypeStyle::Mono, 0),
        };
        let text = &self.text;
        let mut measure = |t: &str| text.measure(style, t).0;
        let lines = chat::wrap(&entry.text, (width - indent) as f32, &mut measure);
        Laid {
            who: entry.who,
            label,
            style,
            lines,
        }
    }

    fn entry(
        &mut self,
        c: &mut Canvas<'_>,
        clip: Rect,
        x: i32,
        top: i32,
        laid: &Laid,
        (label_h, line_h): (i32, i32),
    ) {
        let (label_color, color, indent) = match laid.who {
            Who::You => (self.palette.dim, self.palette.muted, 0),
            Who::Kai => (self.palette.ai, self.palette.text, RULE + INDENT),
            Who::Note => (self.palette.dim, self.palette.dim, 0),
        };
        let mut y = top;
        if !laid.label.is_empty() {
            let base = self.baseline(y, label_h, TypeStyle::Small);
            self.draw_text(
                c,
                clip,
                x as f32,
                base,
                TypeStyle::Small,
                label_color,
                laid.label,
            );
            y += label_h;
        }
        if laid.who == Who::Kai {
            let h = line_h * laid.lines.len() as i32;
            fill(c, Rect::new(x, y, RULE, h), clip, self.palette.ai);
        }
        for line in &laid.lines {
            let base = self.baseline(y, line_h, laid.style);
            self.draw_text(c, clip, (x + indent) as f32, base, laid.style, color, line);
            y += line_h;
        }
    }
}
