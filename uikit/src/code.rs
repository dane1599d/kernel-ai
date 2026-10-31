//! The code view (`kai:code`): a grid of mono text the app owns, with a gutter,
//! coloured runs, a caret and a highlighted row. Its geometry (one cell is the
//! mono style's advance of `0`, one row its line height), its drawing and the
//! names its keys go by live here; routing is `route.rs`'s, as for any widget.

use crate::geom::{Point, Rect};
use crate::input::Key;
use crate::layout::Measure;
use crate::list::Op;
use crate::style::{Token, TypeStyle, metric::*};
use crate::widget::{CodeRow, CodeView};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Most bytes of text the accessibility tree carries for a code view.
pub const A11Y_BYTES: usize = 4096;

/// One cell's width in px.
pub fn cell_w(m: &dyn Measure) -> i32 {
    m.advance(TypeStyle::Mono, "0").max(1)
}

/// One row's height in px.
pub fn row_h(m: &dyn Measure) -> i32 {
    m.line(TypeStyle::Mono).max(1)
}

/// The size a code view would like: all its rows at full width.
pub fn natural(m: &dyn Measure, v: &CodeView) -> (i32, i32) {
    let widest = v
        .rows
        .iter()
        .map(|r| r.text.chars().count())
        .max()
        .unwrap_or(0);
    let cells = i32::from(v.gutter_width) + widest as i32;
    (
        cells * cell_w(m) + 2 * CODE_PAD,
        v.rows.len() as i32 * row_h(m) + 2 * CODE_PAD,
    )
}

/// How many rows, and how many text cells after the gutter, fit a view placed at
/// `r`: what the app needs to know to fill it.
pub fn grid(m: &dyn Measure, r: Rect, gutter: u16) -> (u16, u16) {
    let rows = ((r.h - 2 * CODE_PAD) / row_h(m)).max(0);
    let cells = ((r.w - 2 * CODE_PAD) / cell_w(m) - i32::from(gutter)).max(0);
    (
        u16::try_from(rows).unwrap_or(u16::MAX),
        u16::try_from(cells).unwrap_or(u16::MAX),
    )
}

/// The row and text cell under `p` in a view placed at `r`; a point in the
/// gutter or above the first row gives 0.
pub fn at(m: &dyn Measure, r: Rect, gutter: u16, p: Point) -> (u32, u32) {
    let row = ((p.y - r.y - CODE_PAD) / row_h(m)).max(0);
    let cell = ((p.x - r.x - CODE_PAD) / cell_w(m) - i32::from(gutter)).max(0);
    (row as u32, cell as u32)
}

/// What the accessibility tree says a code view holds: its rows, cut to
/// [`A11Y_BYTES`].
pub fn a11y_value(v: &CodeView) -> String {
    let mut out = String::new();
    for row in &v.rows {
        if out.len() + row.text.len() + 1 > A11Y_BYTES {
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&row.text);
    }
    out
}

/// The name a key goes by in `kai:code`: `enter`, `esc`, `backspace`, `up`,
/// `pagedown`, `f1`, a letter or digit as itself, punctuation as the character
/// the key types without shift, and `hid-0x..` for the rest.
pub fn key_name(key: Key) -> String {
    let named = match key {
        Key::Enter => "enter",
        Key::Escape => "esc",
        Key::Backspace => "backspace",
        Key::Tab => "tab",
        Key::Space => "space",
        Key::Delete => "delete",
        Key::Left => "left",
        Key::Right => "right",
        Key::Up => "up",
        Key::Down => "down",
        Key::Home => "home",
        Key::End => "end",
        Key::PageUp => "pageup",
        Key::PageDown => "pagedown",
        Key::Other(usage) => return hid_name(usage),
    };
    String::from(named)
}

fn hid_name(usage: u16) -> String {
    match usage {
        0x04..=0x1D => String::from(char::from(b'a' + (usage - 0x04) as u8)),
        0x1E..=0x26 => String::from(char::from(b'1' + (usage - 0x1E) as u8)),
        0x27 => String::from("0"),
        0x2D => String::from("-"),
        0x2E => String::from("="),
        0x2F => String::from("["),
        0x30 => String::from("]"),
        0x31 => String::from("\\"),
        0x33 => String::from(";"),
        0x34 => String::from("'"),
        0x35 => String::from("`"),
        0x36 => String::from(","),
        0x37 => String::from("."),
        0x38 => String::from("/"),
        0x3A..=0x45 => format!("f{}", usage - 0x39),
        other => format!("hid-{other:#x}"),
    }
}

/// Whether `key` is a modifier on its own (Ctrl, Shift, Alt, Super, either
/// side): the app learns of those through the modifiers the next key carries.
pub fn is_modifier(key: Key) -> bool {
    matches!(key, Key::Other(0xE0..=0xE7))
}

fn text_op(x: i32, y: i32, token: Token, text: &str) -> Op {
    Op::Text {
        x,
        y,
        style: TypeStyle::Mono,
        token,
        text: String::from(text),
    }
}

/// `row`'s text split into the runs `spans` colour and the gaps between them.
fn runs(row: &CodeRow) -> Vec<(usize, String, Token, bool)> {
    let chars: Vec<char> = row.text.chars().collect();
    let mut spans: Vec<_> = row.spans.iter().collect();
    spans.sort_by_key(|s| s.from);
    let mut out = Vec::new();
    let mut pos = 0usize;
    for s in spans {
        let (from, to) = ((s.from as usize).max(pos), (s.to as usize).min(chars.len()));
        if from >= to {
            continue;
        }
        if from > pos {
            out.push((pos, chars[pos..from].iter().collect(), Token::Text, false));
        }
        let token = if s.link {
            Token::Accent
        } else {
            s.tone.token()
        };
        out.push((from, chars[from..to].iter().collect(), token, s.link));
        pos = to;
    }
    if pos < chars.len() {
        out.push((pos, chars[pos..].iter().collect(), Token::Text, false));
    }
    out
}

/// What the view draws at `r`; the caret only while `focused`.
pub fn ops(m: &dyn Measure, v: &CodeView, r: Rect, focused: bool) -> Vec<Op> {
    let (cw, rh) = (cell_w(m), row_h(m));
    let mut ops = alloc::vec![
        Op::Fill {
            rect: r,
            token: Token::Desk,
            radius: RADIUS,
        },
        Op::Stroke {
            rect: r,
            token: Token::LineStrong,
            width: 1,
            radius: RADIUS,
        },
        Op::PushClip(r.inset(1)),
    ];
    let (x0, y0) = (r.x + CODE_PAD, r.y + CODE_PAD);
    let text_x = x0 + i32::from(v.gutter_width) * cw;
    for (i, row) in v.rows.iter().enumerate() {
        let y = y0 + i as i32 * rh;
        if y >= r.y + r.h {
            break;
        }
        if v.selected == Some(i as u32) {
            ops.push(Op::Fill {
                rect: Rect::new(r.x + 1, y, r.w - 2, rh),
                token: Token::Raised,
                radius: 0,
            });
        }
        if !row.gutter.is_empty() {
            // Right-aligned, with a blank cell between it and the text.
            let used = row.gutter.chars().count() as i32 + 1;
            let gx = x0 + (i32::from(v.gutter_width) - used).max(0) * cw;
            ops.push(text_op(gx, y, Token::Dim, &row.gutter));
        }
        for (at, text, token, link) in runs(row) {
            let x = text_x + at as i32 * cw;
            ops.push(text_op(x, y, token, &text));
            if link {
                let w = text.chars().count() as i32 * cw;
                ops.push(Op::Fill {
                    rect: Rect::new(x, y + rh - 2, w, 1),
                    token,
                    radius: 0,
                });
            }
        }
        if focused && v.cursor.is_some_and(|(row, _)| row == i as u32) {
            let cell = v.cursor.map_or(0, |(_, cell)| cell) as i32;
            ops.push(Op::Fill {
                rect: Rect::new(text_x + cell * cw, y, 2, rh),
                token: Token::Text,
                radius: 0,
            });
        }
    }
    ops.push(Op::PopClip);
    ops
}
