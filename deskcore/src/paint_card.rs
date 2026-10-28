//! The ask card's drawing (ADR 0005 §7, §8): a `raised` sheet above the panel
//! with a 3 px `ask` bar, a title and the 2:00 countdown, the call in mono as
//! the kernel hashed it (wrapped, never cut), the reason, and Run (y) and
//! Don't (n). It sits in the trusted layer, above every window.

use crate::askcard::Card;
use crate::geom::Rect;
use crate::paint::{Painter, baseline, fill, g};
use crate::trusted::{self, Ask, LINE, PADDING};
use alloc::vec::Vec;
use gfx2d::Canvas;
use look::{DARK, TypeStyle};

const TITLE: &str = "Kai asks to run";
const YES: &str = "Run (y)";
const NO: &str = "Don't (n)";
/// The `ask` bar along the card's top.
const BAR: i32 = 3;

impl Painter {
    /// Characters of mono text a line of the card holds on a `width` px screen.
    pub fn card_cols(&self, width: i32) -> usize {
        let (advance, _, _) = self.text.cell(1.0);
        let inner = (width - 2 * PADDING).min(trusted::SHEET_MAX) - 2 * PADDING;
        (inner as f32 / advance).max(1.0) as usize
    }

    /// The lines `card` takes on a `width` px screen, for `Desk::show_ask`: its
    /// title, its call and its reason.
    pub fn card_lines(&self, width: i32, card: &Card) -> i32 {
        let (call, reason) = card.lines(self.card_cols(width));
        (1 + call.len() + reason.len()) as i32
    }

    pub(crate) fn ask_card(
        &mut self,
        c: &mut Canvas<'_>,
        clip: Rect,
        ask: &Ask,
        card: &Card,
        now_ms: u64,
    ) {
        let (width, height) = (c.width as i32, c.height as i32);
        let area = trusted::card(width, height, ask);
        let Some(clip) = area.intersect(&clip) else {
            return;
        };
        fill(c, area, clip, DARK.raised);
        fill(c, Rect::new(area.x, area.y, area.w, BAR), clip, DARK.ask);
        let inner = Rect::new(
            area.x + PADDING,
            area.y + PADDING,
            area.w - 2 * PADDING,
            area.h - 2 * PADDING,
        );
        let text_clip = inner.intersect(&clip).unwrap_or_default();
        let mut top = inner.y;
        let y = baseline(top, LINE, TypeStyle::Label);
        self.draw_text(
            c,
            text_clip,
            inner.x as f32,
            y,
            TypeStyle::Label,
            DARK.text,
            TITLE,
        );
        let left = card.left(now_ms);
        let (w, _) = self.text.measure(TypeStyle::MonoTitle, &left);
        let x = inner.right() as f32 - w;
        self.draw_text(c, text_clip, x, y, TypeStyle::MonoTitle, DARK.dim, &left);
        top += LINE;
        let (call, reason) = card.lines(self.card_cols(width));
        let rows: Vec<(&str, bool)> = call
            .iter()
            .map(|l| (l.as_str(), true))
            .chain(reason.iter().map(|l| (l.as_str(), false)))
            .collect();
        for (line, mono) in rows {
            if top + LINE > inner.bottom() - trusted::BUTTON_H {
                break;
            }
            let y = baseline(top, LINE, TypeStyle::Mono);
            let x = inner.x as f32;
            match mono {
                true => {
                    self.text
                        .draw_mono(c, g(text_clip), x, y, 1.0, false, DARK.text, line);
                }
                false => {
                    self.draw_text(c, text_clip, x, y, TypeStyle::Body, DARK.muted, line);
                }
            }
            top += LINE;
        }
        let (yes, no) = trusted::buttons(area);
        self.button(c, clip, yes, YES, true);
        self.button(c, clip, no, NO, false);
    }

    /// A card button: Run filled in `ask`, Don't framed in `line_strong`.
    fn button(&mut self, c: &mut Canvas<'_>, clip: Rect, r: Rect, label: &str, primary: bool) {
        let ink = match primary {
            true => {
                fill(c, r, clip, DARK.ask);
                DARK.window
            }
            false => {
                for edge in [
                    Rect::new(r.x, r.y, r.w, 1),
                    Rect::new(r.x, r.bottom() - 1, r.w, 1),
                    Rect::new(r.x, r.y, 1, r.h),
                    Rect::new(r.right() - 1, r.y, 1, r.h),
                ] {
                    fill(c, edge, clip, DARK.line_strong);
                }
                DARK.text
            }
        };
        let (w, _) = self.text.measure(TypeStyle::Label, label);
        let x = r.x as f32 + (r.w as f32 - w) / 2.0;
        let y = baseline(r.y, r.h, TypeStyle::Label);
        let inside = r.intersect(&clip).unwrap_or_default();
        self.draw_text(c, inside, x, y, TypeStyle::Label, ink, label);
    }
}
