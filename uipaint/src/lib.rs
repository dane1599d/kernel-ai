//! `uikit`'s display list drawn with `gfx2d` (ADR 0005 sections 4-5): what
//! comphost puts in a window's buffer. Tokens become a `look` palette's colours,
//! logical px become buffer px at `scale / 120`, rounded only at edges, and text
//! goes through `gfx2d`'s shaper and atlas. [`Measured`] gives `uikit` the same
//! fonts' advances, so the layout and the pixels agree.
//!
//! Not yet drawn as themselves: icons (no Lucide set in the boot bundle yet) are
//! a 1 px frame in their token, and images are a `raised` box.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;

use alloc::vec::Vec;
use gfx2d::text::Text;
use gfx2d::{Canvas, Rect};
use look::Palette;
use uikit::geom::Rect as Logical;
use uikit::layout::Measure;
use uikit::list::{DisplayList, Op};
use uikit::style::{Token, TypeStyle};

/// `surface.preferred_scale`'s unit: 120 is 1x.
pub const SCALE_ONE: u32 = 120;
const OPAQUE: u32 = 0xFF00_0000;

/// `gfx2d`'s measure as `uikit`'s, in logical px.
pub struct Measured<'a>(pub &'a Text);

impl Measure for Measured<'_> {
    fn measure(&self, style: TypeStyle, text: &str) -> (f32, f32) {
        self.0.measure(look_style(style), text)
    }
}

pub fn look_token(t: Token) -> look::Token {
    match t {
        Token::Desk => look::Token::Desk,
        Token::Window => look::Token::Window,
        Token::Raised => look::Token::Raised,
        Token::Line => look::Token::Line,
        Token::LineStrong => look::Token::LineStrong,
        Token::Text => look::Token::Text,
        Token::Muted => look::Token::Muted,
        Token::Dim => look::Token::Dim,
        Token::Error => look::Token::Error,
        Token::Ok => look::Token::Ok,
        Token::Accent => look::Token::Accent,
        Token::Focus => look::Token::Focus,
    }
}

pub fn look_style(s: TypeStyle) -> look::TypeStyle {
    match s {
        TypeStyle::Small => look::TypeStyle::Small,
        TypeStyle::Body => look::TypeStyle::Body,
        TypeStyle::Label => look::TypeStyle::Label,
        TypeStyle::Heading => look::TypeStyle::Heading,
        TypeStyle::Display => look::TypeStyle::Display,
        TypeStyle::Mono => look::TypeStyle::Mono,
        TypeStyle::MonoTitle => look::TypeStyle::MonoTitle,
    }
}

/// Logical px to buffer px at one scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale(pub f32);

impl Scale {
    pub fn of(scale: u32) -> Self {
        Self(scale.max(1) as f32 / SCALE_ONE as f32)
    }

    fn px(self, v: i32) -> i32 {
        libm::roundf(v as f32 * self.0) as i32
    }

    /// Both edges rounded, so neighbours share their edge.
    pub fn rect(self, r: Logical) -> Rect {
        let (x0, y0) = (self.px(r.x), self.px(r.y));
        Rect::new(x0, y0, self.px(r.x + r.w) - x0, self.px(r.y + r.h) - y0)
    }

    fn len(self, v: i32) -> i32 {
        if v <= 0 { 0 } else { self.px(v).max(1) }
    }
}

/// The row spans of `r` with corners of radius `radius` px: for each row, how
/// far its ends are inset, and the coverage (0-255) of the pixel just inside.
fn insets(r: Rect, radius: i32) -> Vec<(i32, u32)> {
    let radius = radius.clamp(0, r.w.min(r.h) / 2);
    (0..r.h)
        .map(|y| {
            let from_edge = y.min(r.h - 1 - y);
            if from_edge >= radius {
                return (0, 255);
            }
            let rf = radius as f32;
            let dy = rf - from_edge as f32 - 0.5;
            let inset = rf - libm::sqrtf((rf * rf - dy * dy).max(0.0));
            let whole = libm::floorf(inset);
            (whole as i32, (255.0 * (1.0 - (inset - whole))) as u32)
        })
        .collect()
}

/// One row from `x0` to `x1`, its end pixels at coverage `edge` when the row
/// is inset by a corner.
fn span(
    c: &mut Canvas<'_>,
    y: i32,
    (x0, x1): (i32, i32),
    edge: Option<u32>,
    color: u32,
    clip: Rect,
) {
    let Some(edge) = edge else {
        c.fill(Rect::new(x0, y, x1 - x0, 1).intersect(&clip), color);
        return;
    };
    c.fill(Rect::new(x0 + 1, y, x1 - x0 - 2, 1).intersect(&clip), color);
    for x in [x0, x1 - 1] {
        c.fill_alpha(Rect::new(x, y, 1, 1).intersect(&clip), color, edge);
    }
}

/// `r` filled in `color`, its corners rounded and antialiased, as far as `clip`.
fn fill(c: &mut Canvas<'_>, r: Rect, radius: i32, color: u32, clip: Rect) {
    for (y, (inset, edge)) in insets(r, radius).into_iter().enumerate() {
        let ends = (r.x + inset, r.right() - inset);
        let edge = (radius > 0 && in_corner(r, radius, y)).then_some(edge);
        span(c, r.y + y as i32, ends, edge, color, clip);
    }
}

/// Whether row `y` of `r` falls in a rounded corner.
fn in_corner(r: Rect, radius: i32, y: usize) -> bool {
    let y = y as i32;
    y.min(r.h - 1 - y) < radius.clamp(0, r.w.min(r.h) / 2)
}

/// A `width` px outline just inside `r`, its corners rounded.
fn stroke(c: &mut Canvas<'_>, r: Rect, width: i32, radius: i32, color: u32, clip: Rect) {
    let inner = r.grow(-width);
    let inner_radius = (radius - width).max(0);
    let inner_rows = insets(inner, inner_radius);
    for (y, (inset, edge)) in insets(r, radius).into_iter().enumerate() {
        let row = r.y + y as i32;
        let (x0, x1) = (r.x + inset, r.right() - inset);
        let edge = (radius > 0 && in_corner(r, radius, y)).then_some(edge);
        let at = row - inner.y;
        if inner.is_empty() || at < 0 || at >= inner.h {
            span(c, row, (x0, x1), edge, color, clip);
            continue;
        }
        let ii = inner_rows[at as usize].0;
        // Each side on its own: the outer end antialiased, the inner end hard.
        let hard = i32::from(edge.is_some());
        let (left, right) = ((x0 + hard, inner.x + ii), (inner.right() - ii, x1 - hard));
        for (a, b) in [left, right] {
            c.fill(Rect::new(a, row, b - a, 1).intersect(&clip), color);
        }
        if let Some(edge) = edge {
            for x in [x0, x1 - 1] {
                c.fill_alpha(Rect::new(x, row, 1, 1).intersect(&clip), color, edge);
            }
        }
    }
}

/// The baseline's offset from the top of a line box `line` px tall for text of
/// `size` px: the font's ascent and descent centred in the line. Inter's and
/// JetBrains Mono's ascents and descents, as fractions of the em.
fn baseline(style: TypeStyle, size: f32, line: f32) -> f32 {
    let (ascent, descent) = match style {
        TypeStyle::Mono | TypeStyle::MonoTitle => (1.02, 0.3),
        _ => (0.969, 0.242),
    };
    (line - (ascent + descent) * size) / 2.0 + ascent * size
}

/// What one frame draws with.
pub struct Painter<'a> {
    pub palette: &'a Palette,
    pub text: &'a mut Text,
    pub scale: Scale,
}

impl Painter<'_> {
    fn color(&self, t: Token) -> u32 {
        self.palette.get(look_token(t)) | OPAQUE
    }

    /// Draws `list` into `c`, as far as `damage` (buffer px) reaches: the
    /// window's background first, then each group in its clip.
    pub fn paint(&mut self, c: &mut Canvas<'_>, list: &DisplayList, damage: Rect) {
        let area = damage.intersect(&c.bounds());
        c.fill(area, self.color(Token::Window));
        for g in &list.groups {
            let mut clips = alloc::vec![area.intersect(&self.scale.rect(g.clip))];
            if clips[0].is_empty() {
                continue;
            }
            for op in &g.ops {
                let clip = *clips.last().unwrap_or(&area);
                match op {
                    Op::PushClip(r) => clips.push(clip.intersect(&self.scale.rect(*r))),
                    Op::PopClip => {
                        if clips.len() > 1 {
                            clips.pop();
                        }
                    }
                    _ => self.op(c, op, clip),
                }
            }
        }
    }

    fn op(&mut self, c: &mut Canvas<'_>, op: &Op, clip: Rect) {
        let s = self.scale;
        match op {
            Op::Fill {
                rect,
                token,
                radius,
            } => fill(c, s.rect(*rect), s.len(*radius), self.color(*token), clip),
            Op::Stroke {
                rect,
                token,
                width,
                radius,
            } => stroke(
                c,
                s.rect(*rect),
                s.len(*width),
                s.len(*radius),
                self.color(*token),
                clip,
            ),
            Op::Text {
                x,
                y,
                style,
                token,
                text,
            } => {
                let t = look::style(look_style(*style), self.text.legible);
                let top = *y as f32 * s.0;
                let base = top + baseline(*style, t.size, t.line) * s.0;
                let color = self.palette.get(look_token(*token));
                self.text.draw(
                    c,
                    clip,
                    *x as f32 * s.0,
                    libm::roundf(base) as i32,
                    look_style(*style),
                    s.0,
                    color,
                    text,
                );
            }
            Op::Icon { rect, token, .. } => {
                let r = s.rect(*rect).intersect(&clip);
                if !r.is_empty() {
                    c.frame(r, self.color(*token));
                }
            }
            Op::Image { rect, .. } => {
                c.fill(s.rect(*rect).intersect(&clip), self.color(Token::Raised))
            }
            Op::PushClip(_) | Op::PopClip => {}
        }
    }
}
