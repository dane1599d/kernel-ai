//! How a Kai glyph is authored: strokes and fills built on the 24-unit grid,
//! y down, as `tiny-skia` paths.

use alloc::vec::Vec;
use tiny_skia::{Path, PathBuilder};

/// A path stroked at [`STROKE`], or filled.
pub(super) enum Part {
    Stroke(Path),
    Fill(Path),
}

/// A glyph being authored, in grid units, y down.
#[derive(Default)]
pub(super) struct Glyph {
    pub(super) parts: Vec<Part>,
}

impl Glyph {
    pub(super) fn stroke(&mut self, pb: PathBuilder) {
        if let Some(p) = pb.finish() {
            self.parts.push(Part::Stroke(p));
        }
    }

    pub(super) fn fill(&mut self, pb: PathBuilder) {
        if let Some(p) = pb.finish() {
            self.parts.push(Part::Fill(p));
        }
    }

    /// An open polyline through `pts`.
    pub(super) fn lines(&mut self, pts: &[(f32, f32)]) {
        let mut pb = PathBuilder::new();
        for (i, &(x, y)) in pts.iter().enumerate() {
            match i {
                0 => pb.move_to(x, y),
                _ => pb.line_to(x, y),
            }
        }
        self.stroke(pb);
    }

    /// A closed polygon through `pts`.
    pub(super) fn closed(&mut self, pts: &[(f32, f32)]) {
        let mut pb = PathBuilder::new();
        for (i, &(x, y)) in pts.iter().enumerate() {
            match i {
                0 => pb.move_to(x, y),
                _ => pb.line_to(x, y),
            }
        }
        pb.close();
        self.stroke(pb);
    }

    /// A filled dot.
    pub(super) fn dot(&mut self, x: f32, y: f32, r: f32) {
        let mut pb = PathBuilder::new();
        pb.push_circle(x, y, r);
        self.fill(pb);
    }

    /// An arc of radius `r` about (`cx`, `cy`) from `a0` to `a1` degrees,
    /// counter-clockwise as seen, 0 at the right.
    pub(super) fn arc(&mut self, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) {
        let mut pb = PathBuilder::new();
        arc_to(&mut pb, cx, cy, r, a0, a1, true);
        if a1 - a0 >= 360.0 {
            pb.close();
        }
        self.stroke(pb);
    }

    /// The outline of a rectangle from (x0, y0) to (x1, y1), its corners
    /// rounded by `r`.
    pub(super) fn round_rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) {
        self.stroke(round_rect(x0, y0, x1, y1, r));
    }

    pub(super) fn fill_round_rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) {
        self.fill(round_rect(x0, y0, x1, y1, r));
    }

    /// A folder: a body with a tab on its upper left.
    pub(super) fn folder(&mut self) {
        let mut pb = PathBuilder::new();
        pb.move_to(3.0, 7.0);
        pb.line_to(3.0, 17.5);
        pb.quad_to(3.0, 19.0, 4.5, 19.0);
        pb.line_to(19.5, 19.0);
        pb.quad_to(21.0, 19.0, 21.0, 17.5);
        pb.line_to(21.0, 9.5);
        pb.quad_to(21.0, 8.0, 19.5, 8.0);
        pb.line_to(12.0, 8.0);
        pb.line_to(10.0, 5.5);
        pb.line_to(4.5, 5.5);
        pb.quad_to(3.0, 5.5, 3.0, 7.0);
        pb.close();
        self.stroke(pb);
    }

    /// A page with its upper right corner folded, and two lines of text.
    pub(super) fn page(&mut self) {
        let mut pb = PathBuilder::new();
        pb.move_to(6.5, 3.0);
        pb.line_to(14.0, 3.0);
        pb.line_to(19.0, 8.0);
        pb.line_to(19.0, 19.5);
        pb.quad_to(19.0, 21.0, 17.5, 21.0);
        pb.line_to(6.5, 21.0);
        pb.quad_to(5.0, 21.0, 5.0, 19.5);
        pb.line_to(5.0, 4.5);
        pb.quad_to(5.0, 3.0, 6.5, 3.0);
        pb.close();
        self.stroke(pb);
        self.lines(&[(14.0, 3.0), (14.0, 8.0), (19.0, 8.0)]);
        self.lines(&[(8.5, 13.0), (15.5, 13.0)]);
        self.lines(&[(8.5, 17.0), (13.0, 17.0)]);
    }

    /// A curly brace whose ends sit at `x`, opening to the right when `dir`
    /// is -1 (`{`) and to the left when it is 1 (`}`).
    pub(super) fn brace(&mut self, x: f32, dir: f32) {
        let (inner, tip) = (x + 3.0 * dir, x + 5.0 * dir);
        let mut pb = PathBuilder::new();
        pb.move_to(x, 4.0);
        pb.quad_to(inner, 4.0, inner, 7.0);
        pb.line_to(inner, 10.0);
        pb.quad_to(inner, 12.0, tip, 12.0);
        pb.quad_to(inner, 12.0, inner, 14.0);
        pb.line_to(inner, 17.0);
        pb.quad_to(inner, 20.0, x, 20.0);
        self.stroke(pb);
    }

    /// A ring with six notches round it, and a hub.
    pub(super) fn gear(&mut self) {
        self.arc(12.0, 12.0, 6.0, 0.0, 360.0);
        self.arc(12.0, 12.0, 2.25, 0.0, 360.0);
        for k in 0..6 {
            let a = (k as f32 * 60.0 + 30.0).to_radians();
            let (c, s) = (libm::cosf(a), libm::sinf(a));
            self.lines(&[
                (12.0 + 6.0 * c, 12.0 - 6.0 * s),
                (12.0 + 9.0 * c, 12.0 - 9.0 * s),
            ]);
        }
    }

    /// A speech bubble with its tail at the lower left.
    pub(super) fn bubble(&mut self) {
        let mut pb = PathBuilder::new();
        pb.move_to(7.0, 4.5);
        pb.line_to(17.0, 4.5);
        pb.quad_to(20.0, 4.5, 20.0, 7.5);
        pb.line_to(20.0, 13.0);
        pb.quad_to(20.0, 16.0, 17.0, 16.0);
        pb.line_to(11.5, 16.0);
        pb.line_to(7.5, 19.5);
        pb.line_to(7.5, 16.0);
        pb.line_to(7.0, 16.0);
        pb.quad_to(4.0, 16.0, 4.0, 13.0);
        pb.line_to(4.0, 7.5);
        pb.quad_to(4.0, 4.5, 7.0, 4.5);
        pb.close();
        self.stroke(pb);
    }

    /// A chip with three pins on each side.
    pub(super) fn chip(&mut self) {
        self.round_rect(6.0, 6.0, 18.0, 18.0, 1.5);
        self.round_rect(9.5, 9.5, 14.5, 14.5, 0.5);
        for p in [9.0, 12.0, 15.0] {
            self.lines(&[(p, 3.0), (p, 6.0)]);
            self.lines(&[(p, 18.0), (p, 21.0)]);
            self.lines(&[(3.0, p), (6.0, p)]);
            self.lines(&[(18.0, p), (21.0, p)]);
        }
    }

    /// The window behind, in [`Icon::Restore`]: its upper right corner.
    pub(super) fn restore_back(&mut self) {
        let mut pb = PathBuilder::new();
        pb.move_to(9.0, 9.0);
        pb.line_to(9.0, 6.0);
        pb.quad_to(9.0, 5.0, 10.0, 5.0);
        pb.line_to(18.0, 5.0);
        pb.quad_to(19.0, 5.0, 19.0, 6.0);
        pb.line_to(19.0, 14.0);
        pb.quad_to(19.0, 15.0, 18.0, 15.0);
        pb.line_to(15.0, 15.0);
        self.stroke(pb);
    }
}

/// A rectangle's outline with rounded corners.
fn round_rect(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> PathBuilder {
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + r, y0);
    pb.line_to(x1 - r, y0);
    pb.quad_to(x1, y0, x1, y0 + r);
    pb.line_to(x1, y1 - r);
    pb.quad_to(x1, y1, x1 - r, y1);
    pb.line_to(x0 + r, y1);
    pb.quad_to(x0, y1, x0, y1 - r);
    pb.line_to(x0, y0 + r);
    pb.quad_to(x0, y0, x0 + r, y0);
    pb.close();
    pb
}

/// Appends an arc to `pb` in cubic pieces of at most 90 degrees, starting it
/// with a move when `start` is set.
fn arc_to(pb: &mut PathBuilder, cx: f32, cy: f32, r: f32, a0: f32, a1: f32, start: bool) {
    let at = |a: f32| {
        let a = a.to_radians();
        (cx + r * libm::cosf(a), cy - r * libm::sinf(a))
    };
    // The tangent, per radian, at `a`.
    let tangent = |a: f32| {
        let a = a.to_radians();
        (-r * libm::sinf(a), -r * libm::cosf(a))
    };
    let pieces = libm::ceilf((a1 - a0) / 90.0).max(1.0);
    let step = (a1 - a0) / pieces;
    let k = 4.0 / 3.0 * libm::tanf(step.to_radians() / 4.0);
    let (x, y) = at(a0);
    if start {
        pb.move_to(x, y);
    }
    for i in 0..pieces as i32 {
        let (s, e) = (a0 + step * i as f32, a0 + step * (i + 1) as f32);
        let ((sx, sy), (ex, ey)) = (at(s), at(e));
        let ((tsx, tsy), (tex, tey)) = (tangent(s), tangent(e));
        pb.cubic_to(
            sx + k * tsx,
            sy + k * tsy,
            ex - k * tex,
            ey - k * tey,
            ex,
            ey,
        );
    }
}
