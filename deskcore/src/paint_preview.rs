//! A send's text in the Kai pane while its card is up (ADR 0005 §7,
//! "Amended 2026-11-19"): in the history rectangle, in place of the
//! history, in the AI's look (the `Kai` label in `ai`, its 2 px rule, sans
//! `text`), since the text is the AI's words; never in the card and never in
//! the trusted look. Over it is a header code writes, mono `dim` as the
//! status row's, wrapped so all of it shows: how many bytes go where, then
//! the digest the card shows and which lines show. The text is wrapped by
//! character, every one kept, spaces at a line's ends and empty lines
//! marked ([`SendPreview::shown`]). Only a [`SendPreview`] reaches here,
//! whose text `desktop` has checked against the card's digest.

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

/// `line` cut into pieces at most `width` px wide by `measure`, character by
/// character, every character kept; at least one character a piece.
fn wrap_chars(line: &str, width: f32, measure: &dyn Fn(&str) -> f32) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut piece = String::new();
    for ch in line.chars() {
        piece.push(ch);
        if piece.chars().count() > 1 && measure(&piece) > width {
            piece.pop();
            pieces.push(core::mem::take(&mut piece));
            piece.push(ch);
        }
    }
    pieces.push(piece);
    pieces
}

/// A preview laid out in an area: the header's lines, the text's lines, and
/// how many of the text's lines fit at once.
pub struct PreviewLayout {
    pub header: Vec<String>,
    pub lines: Vec<String>,
    pub rows: usize,
}

impl Painter {
    /// `preview` laid out in `area`: the header wrapped in mono, the text
    /// wrapped by character in the AI's sans, and how many text lines fit
    /// under the header and the `Kai` label.
    pub fn preview_layout(&self, area: Rect, preview: &SendPreview) -> PreviewLayout {
        let text = &self.text;
        let body = |t: &str| text.measure(TypeStyle::Body, t).0;
        let mono = |t: &str| text.measure(TypeStyle::Mono, t).0;
        let width = (area.w - RULE - INDENT).max(1) as f32;
        let lines: Vec<String> = preview
            .shown()
            .iter()
            .flat_map(|line| wrap_chars(line, width, &body))
            .collect();
        let line_h = self.text.measure(TypeStyle::Body, "x").1 as i32;
        let head_h = self.text.measure(TypeStyle::Mono, "x").1 as i32;
        let label = self.text.measure(TypeStyle::Small, "Kai").1 as i32;
        // The header's own lines take as many rows as they wrap to; the
        // count it shows is of the rows left for the text.
        let wrap_header = |rows: usize| -> Vec<String> {
            preview
                .header(rows, lines.len())
                .iter()
                .flat_map(|h| wrap_chars(h, area.w.max(1) as f32, &mono))
                .collect()
        };
        let guess = wrap_header(lines.len()).len() as i32;
        let rows = ((area.h - guess * head_h - label).max(0) / line_h.max(1)) as usize;
        let header = wrap_header(rows);
        PreviewLayout {
            header,
            lines,
            rows,
        }
    }

    /// `preview` in `area` of the canvas, nothing drawn outside it.
    pub(crate) fn send_preview(&mut self, c: &mut Canvas<'_>, area: Rect, preview: &SendPreview) {
        if area.is_empty() {
            return;
        }
        let laid = self.preview_layout(area, preview);
        let head_h = self.text.measure(TypeStyle::Mono, "x").1 as i32;
        let dim = self.palette.dim;
        let mut y = area.y;
        for line in &laid.header {
            let base = self.baseline(y, head_h, TypeStyle::Mono);
            self.draw_text(c, area, area.x as f32, base, TypeStyle::Mono, dim, line);
            y += head_h;
        }
        let label = self.text.measure(TypeStyle::Small, "Kai").1 as i32;
        let base = self.baseline(y, label, TypeStyle::Small);
        let ai = self.palette.ai;
        self.draw_text(c, area, area.x as f32, base, TypeStyle::Small, ai, "Kai");
        y += label;
        let line_h = self.text.measure(TypeStyle::Body, "x").1 as i32;
        let shown: Vec<&String> = laid
            .lines
            .iter()
            .skip(preview.scrolled)
            .take(laid.rows)
            .collect();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Wrapping by character keeps every character, spaces included.
    #[test]
    fn wrapping_by_character_keeps_every_character() {
        let measure = |t: &str| t.chars().count() as f32;
        let line = "ab  cd e";
        let pieces = wrap_chars(line, 3.0, &measure);
        assert_eq!(pieces, ["ab ", " cd", " e"]);
        assert_eq!(pieces.concat(), line);
        assert_eq!(wrap_chars("", 3.0, &measure), [""]);
    }

    /// The header shows whole in the pane at every size: where, how much,
    /// the digest and the lines shown (#1142's review: it was clipped).
    #[test]
    fn the_header_shows_whole_at_every_size() {
        use crate::paint_tests::FONTS;
        let painter = Painter::new(FONTS).unwrap();
        let text = "hello from the desk";
        let call = alloc::format!(
            "net_send(\"a-rather-long-host-name.hooks.example.org\", \"/a/longer/path/for/the/hook\", \"{}\")",
            crate::sendview::digest(text)
        );
        let preview = SendPreview::new(3, &call, text).unwrap();
        for (w, h) in [(1024, 768), (1280, 800), (1920, 1080)] {
            let mut desk = crate::desk::Desk::new(w, h);
            desk.set_focus(crate::desk::Focus::Panel, &mut Vec::new());
            let area = desk.layout().pane_parts().history;
            let laid = painter.preview_layout(area, &preview);
            for line in &laid.header {
                let width = painter.text.measure(TypeStyle::Mono, line).0;
                assert!(width <= area.w as f32, "{w}x{h}: {line} is clipped");
            }
            let all = laid.header.concat();
            assert!(
                all.contains(&crate::sendview::digest(text)),
                "{w}x{h}: {all}"
            );
            assert!(all.contains("lines 1-1 of 1"), "{w}x{h}: {all}");
            assert!(all.contains("19 bytes to"), "{w}x{h}: {all}");
        }
    }
}
