//! A send's text in the Kai pane while its card is up (ADR 0005 §7,
//! "Amended 2026-11-19"): in the history rectangle, in place of the
//! history, in the AI's look (the `Kai` label in `ai`, its 2 px rule, in
//! `text`), since the text is the AI's words; never in the card and never in
//! the trusted look. Over it is a header code writes, mono `dim` as the
//! status row's, wrapped so all of it shows: how many bytes go where, then
//! the digest the card shows and which lines show. The text is in mono
//! ([`SEND_STYLE`]), where every character draws as its own glyph (`I`, `l`
//! and `1`, `0` and `O` apart), so two texts never draw the same; it is
//! wrapped by character, every one kept, spaces at a line's ends and empty
//! lines marked ([`SendPreview::shown`]). Only a [`SendPreview`] reaches here,
//! whose text `desktop` has checked against the card's digest.

use crate::geom::Rect;
use crate::paint::{Painter, fill};
use crate::sendview::{SendPreview, wrap_shown};
use alloc::string::String;
use alloc::vec::Vec;
use gfx2d::Canvas;
use look::TypeStyle;

/// The AI's rule, and the space after it, as the history's.
const RULE: i32 = 2;
const INDENT: i32 = 12;

/// The send text's type: mono, whose glyphs tell every printable character
/// apart, where the AI's sans draws `I` and `l` the same (#1133's re-check).
pub const SEND_STYLE: TypeStyle = TypeStyle::Mono;

/// Code's header line `line` cut into pieces no wider than `width`, every
/// character kept: the header is code's own words, so it needs no marks.
fn wrap_code(line: &str, width: f32, measure: &dyn Fn(&str) -> f32) -> Vec<String> {
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
    /// wrapped by character in [`SEND_STYLE`], and how many text lines fit
    /// under the header and the `Kai` label.
    pub fn preview_layout(&self, area: Rect, preview: &SendPreview) -> PreviewLayout {
        let text = &self.text;
        let body = |t: &str| text.measure(SEND_STYLE, t).0;
        let mono = |t: &str| text.measure(TypeStyle::Mono, t).0;
        let width = (area.w - RULE - INDENT).max(1) as f32;
        let lines: Vec<String> = preview
            .shown()
            .iter()
            .flat_map(|line| wrap_shown(line, width, &body))
            .collect();
        let line_h = self.text.measure(SEND_STYLE, "x").1 as i32;
        let head_h = self.text.measure(TypeStyle::Mono, "x").1 as i32;
        let label = self.text.measure(TypeStyle::Small, "Kai").1 as i32;
        // The header's own lines take as many rows as they wrap to; the
        // count it shows is of the rows left for the text.
        let wrap_header = |rows: usize| -> Vec<String> {
            preview
                .header(rows, lines.len())
                .iter()
                .flat_map(|h| wrap_code(h, area.w.max(1) as f32, &mono))
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
        let line_h = self.text.measure(SEND_STYLE, "x").1 as i32;
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
            let base = self.baseline(y, line_h, SEND_STYLE);
            let x = (area.x + RULE + INDENT) as f32;
            self.draw_text(c, area, x, base, SEND_STYLE, text, line);
            y += line_h;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A text, its card and a real layout at every size: the text shows on
    /// the frame after both are here, is read through only once every line
    /// has been on the screen, a jump to the end reads nothing between, and
    /// scrolling through page by page does (#1142's review).
    #[test]
    fn a_long_text_is_read_through_only_line_by_line_on_a_real_layout() {
        use crate::paint_tests::FONTS;
        use crate::sendview::{Slot, digest};
        let painter = Painter::new(FONTS).unwrap();
        let text: String = (0..120)
            .map(|n| alloc::format!("line {n} of the text\n"))
            .collect();
        let call = alloc::format!(
            "net_send(\"hooks.example\", \"/hook\", \"{}\")",
            digest(&text)
        );
        for (w, h) in [(1024, 768), (1280, 800), (1920, 1080)] {
            let mut desk = crate::desk::Desk::new(w, h);
            desk.set_focus(crate::desk::Focus::Panel, &mut Vec::new());
            let area = desk.layout().pane_parts().history;
            let mut slot = Slot::default();
            assert!(slot.card(5, &call).0);
            assert!(slot.text(5, &text).is_some());
            let laid = painter.preview_layout(area, slot.preview().unwrap());
            let (rows, total) = (laid.rows, laid.lines.len());
            assert!(rows > 0 && total > rows, "{w}x{h}: {rows} of {total}");
            assert_eq!(slot.drawn(rows, total), None, "{w}x{h}: the first page");
            // A jump to the end: the last line, nothing between.
            let preview = slot.preview_mut().unwrap();
            preview.scroll(total as i32, total, rows);
            assert_eq!(slot.drawn(rows, total), None, "{w}x{h}: jumped");
            // Back to the top, then page by page.
            slot.preview_mut()
                .unwrap()
                .scroll(-(total as i32), total, rows);
            let mut read = None;
            for _ in 0..total {
                read = read.or(slot.drawn(rows, total));
                let page = rows.saturating_sub(1).max(1) as i32;
                slot.preview_mut().unwrap().scroll(page, total, rows);
            }
            read = read.or(slot.drawn(rows, total));
            assert_eq!(read, Some(5), "{w}x{h}: read through");
        }
    }

    /// Look-alike texts draw differently: `paypaI` and `paypal` are other
    /// glyphs in the send text's type, and so is every printable character,
    /// alone or after a mark, and the marks are real glyphs (#1133's
    /// re-check: in the AI's sans `I` and `l` draw the same).
    #[test]
    fn look_alike_texts_draw_as_different_glyphs() {
        use crate::paint_tests::FONTS;
        use crate::sendview::{EMPTY_MARK, SPACE_MARK, WRAP_MARK};
        let painter = Painter::new(FONTS).unwrap();
        let glyphs = |t: &str| painter.text.glyphs(SEND_STYLE, t);
        assert_ne!(glyphs("paypaI.com"), glyphs("paypal.com"));
        assert_ne!(glyphs("g00gle"), glyphs("gOOgle"));
        for mark in [SPACE_MARK, EMPTY_MARK, WRAP_MARK] {
            let mark = alloc::format!("{mark}");
            let shaped = glyphs(&mark);
            assert!(shaped.len() == 1 && shaped[0] != 0, "{mark}: {shaped:?}");
        }
        for after in ["", "\u{21aa}"] {
            let mut seen = Vec::new();
            for b in 32u8..127 {
                let t = alloc::format!("{after}{}", b as char);
                let last = *glyphs(&t).last().unwrap();
                assert!(last != 0 && !seen.contains(&last), "{t:?} draws as another");
                seen.push(last);
            }
        }
    }

    /// One line wider than the pane at 1920, with a space at the wrap: every
    /// row the pane wrapped onto starts with the wrap mark, the space shows,
    /// and it lays out unlike the same text with a line break there.
    #[test]
    fn a_soft_wrap_never_lays_out_as_a_line_break() {
        use crate::paint_tests::FONTS;
        use crate::sendview::{SPACE_MARK, WRAP_MARK, digest};
        let painter = Painter::new(FONTS).unwrap();
        let mut desk = crate::desk::Desk::new(1920, 1080);
        desk.set_focus(crate::desk::Focus::Panel, &mut Vec::new());
        let area = desk.layout().pane_parts().history;
        let width = (area.w - RULE - INDENT) as f32;
        let fits = (1..400)
            .take_while(|n| {
                let run: String = core::iter::repeat_n('a', *n).collect();
                painter.text.measure(SEND_STYLE, &run).0 <= width
            })
            .count();
        let a: String = core::iter::repeat_n('a', fits).collect();
        let b: String = core::iter::repeat_n('b', fits * 2).collect();
        let layout = |text: &str| {
            let call = alloc::format!("net_send(\"a\", \"/b\", \"{}\")", digest(text));
            let preview = SendPreview::new(1, &call, text).unwrap();
            painter.preview_layout(area, &preview).lines
        };
        let wrapped = layout(&alloc::format!("{a} {b}"));
        assert!(wrapped.len() >= 3, "{wrapped:?}");
        assert_eq!(wrapped[0], a);
        assert!(!wrapped[0].starts_with(WRAP_MARK));
        for row in &wrapped[1..] {
            assert!(row.starts_with(WRAP_MARK), "{row}");
        }
        assert!(
            wrapped[1].starts_with(&alloc::format!("{WRAP_MARK}{SPACE_MARK}")),
            "{}",
            wrapped[1]
        );
        assert_ne!(wrapped, layout(&alloc::format!("{a}\n{b}")));
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
