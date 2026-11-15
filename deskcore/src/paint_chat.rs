//! Drawing the panel's history ([`crate::chat`]) in the Kai pane's history
//! rectangle, newest entry at the bottom, clipped to it so it never reaches
//! the status row, the input, the taskbar or the card. PageUp and PageDown
//! scroll it by entries ([`crate::chat::Chat::scroll`]). Each kind has its
//! own look, so the AI's words never pass for the system's:
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

/// Lines the history scrolls a wheel detent.
pub const WHEEL_LINES: i32 = 3;
/// Space between entries.
const GAP: i32 = 10;
/// The AI's rule, and the space after it.
const RULE: i32 = 2;
const INDENT: i32 = 12;

/// Where the history is drawn on the screen: what a change to it damages.
/// Empty while the pane is closed.
pub fn chat_area(desk: &Desk) -> Rect {
    match desk.pane() {
        Some(_) => desk.layout().pane_parts().history,
        None => Rect::default(),
    }
}

/// One entry, laid out: its label, its lines and their look, kept by the
/// entry's number and the width it was laid out at.
#[derive(Clone)]
pub(crate) struct Laid {
    seq: u64,
    width: i32,
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
    /// The history in `area` of the canvas, nothing drawn outside it.
    pub(crate) fn chat(&mut self, c: &mut Canvas<'_>, area: Rect, scene: &Scene<'_>) {
        if scene.chat.is_empty() || area.is_empty() {
            return;
        }
        let clip = area;
        let label_h = self.text.measure(TypeStyle::Small, "Kai").1 as i32;
        // Scrolled up by px, never past the oldest line at the top.
        let most = (self.chat_height(scene.chat, area.w) - area.h).max(0);
        let mut bottom = area.bottom() + scene.chat.scrolled().clamp(0, most);
        for entry in scene.chat.entries().rev() {
            let laid = self.laid(entry, area.w);
            let line_h = self.text.measure(laid.style, "x").1 as i32;
            let top = bottom - laid.height(label_h, line_h);
            if top < area.bottom() {
                self.entry(c, clip, area.x, top, &laid, (label_h, line_h));
            }
            bottom = top - GAP;
            if bottom <= area.y {
                break;
            }
        }
        // Entries the history let go of are let go here too.
        let first = scene.chat.entries().next().map_or(u64::MAX, |e| e.seq);
        self.chat_laid
            .retain(|l| l.seq >= first && l.width == area.w);
    }

    /// The whole history's height laid out `width` wide, gaps included: how
    /// far it scrolls (`Chat::scroll_by`'s most is this less the area's).
    pub fn chat_height(&mut self, chat: &crate::chat::Chat, width: i32) -> i32 {
        let label_h = self.text.measure(TypeStyle::Small, "Kai").1 as i32;
        let mut total = 0;
        for entry in chat.entries() {
            let laid = self.laid(entry, width);
            let line_h = self.text.measure(laid.style, "x").1 as i32;
            total += laid.height(label_h, line_h) + GAP;
        }
        (total - GAP).max(0)
    }

    /// Adds `text` from `who` to `chat`, drawn in `area`: at the bottom the
    /// new entry shows; scrolled up, the view stays on what it showed, as
    /// the offset grows by the new entry's height and the gap. False when
    /// nothing was added.
    pub fn chat_push(
        &mut self,
        chat: &mut crate::chat::Chat,
        who: crate::chat::Who,
        text: &str,
        area: Rect,
    ) -> bool {
        let scrolled = chat.scrolled() > 0;
        if !chat.push(who, text) {
            return false;
        }
        if scrolled && !area.is_empty() {
            let label_h = self.text.measure(TypeStyle::Small, "Kai").1 as i32;
            let grown = chat.entries().next_back().map_or(0, |entry| {
                let laid = self.laid(entry, area.w);
                let line_h = self.text.measure(laid.style, "x").1 as i32;
                laid.height(label_h, line_h) + GAP
            });
            let most = (self.chat_height(chat, area.w) - area.h).max(0);
            chat.scroll_by(grown, most);
        }
        true
    }

    /// The wheel turned by `detents` over the history in `area`: three lines
    /// a detent, toward the newest for a positive turn (toward the person),
    /// within its content. True when the view moved.
    pub fn wheel_history(
        &mut self,
        chat: &mut crate::chat::Chat,
        area: Rect,
        detents: i32,
    ) -> bool {
        if area.is_empty() {
            return false;
        }
        let line_h = self.text.measure(TypeStyle::Body, "x").1 as i32;
        let most = (self.chat_height(chat, area.w) - area.h).max(0);
        chat.scroll_by(-detents.saturating_mul(WHEEL_LINES * line_h), most)
    }

    /// PageUp's or PageDown's step in a history `area_h` tall: its height
    /// less one line, so a line stays in view across the step.
    pub fn history_page(&self, area_h: i32) -> i32 {
        let line_h = self.text.measure(TypeStyle::Body, "x").1 as i32;
        (area_h - line_h).max(line_h)
    }

    /// `entry` laid out at `width`: kept from an earlier paint, or laid out
    /// now and kept, so a window dragged over the history re-wraps nothing.
    fn laid(&mut self, entry: &Entry, width: i32) -> Laid {
        if let Some(laid) = self
            .chat_laid
            .iter()
            .find(|l| l.seq == entry.seq && l.width == width)
        {
            return laid.clone();
        }
        let laid = self.lay(entry, width);
        self.chat_laid.push(laid.clone());
        laid
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
            seq: entry.seq,
            width,
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
