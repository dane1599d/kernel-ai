//! Putting pixels together for a frame (ADR 0005 §5): `desktop` fills its own
//! surfaces and copies or blends each client's buffer into its frame, inside
//! the frame's damage only. Pixels are `0xAARRGGBB` in sRGB; ARGB8888 buffers
//! are premultiplied, so blending is source-over on premultiplied values with no
//! linear-light step. Lane G-render's `gfx2d` takes over drawing text and
//! chrome; this is the copy and fill path.

use crate::geom::Rect;

/// A frame or a buffer: `height` rows of `stride` pixels, the first `width`
/// of each in use.
pub struct Pixels<'a> {
    pub data: &'a mut [u32],
    pub width: i32,
    pub height: i32,
    pub stride: usize,
}

/// A buffer read from, as [`Pixels`] but shared.
pub struct Source<'a> {
    pub data: &'a [u32],
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    /// Whether the top byte is alpha (premultiplied ARGB8888) or ignored.
    pub alpha: bool,
}

impl Pixels<'_> {
    fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    /// Fills `rect`, cut to the frame and to `clip`.
    pub fn fill(&mut self, rect: Rect, clip: Rect, color: u32) {
        let Some(r) = rect
            .intersect(&clip)
            .and_then(|r| r.intersect(&self.bounds()))
        else {
            return;
        };
        for y in r.y..r.bottom() {
            let row = y as usize * self.stride;
            let start = row + r.x as usize;
            if let Some(span) = self.data.get_mut(start..start + r.w as usize) {
                span.fill(color);
            }
        }
    }

    /// Draws `source` with its top-left corner at `at`, cut to the frame and to
    /// `clip`: copied when it has no alpha, blended source-over when it has.
    pub fn draw(&mut self, source: &Source, at: (i32, i32), clip: Rect) {
        let place = Rect::new(at.0, at.1, source.width, source.height);
        let Some(r) = place
            .intersect(&clip)
            .and_then(|r| r.intersect(&self.bounds()))
        else {
            return;
        };
        for y in r.y..r.bottom() {
            let sy = (y - at.1) as usize;
            let sx = (r.x - at.0) as usize;
            let from = sy * source.stride + sx;
            let to = y as usize * self.stride + r.x as usize;
            let w = r.w as usize;
            let (Some(src), Some(dst)) = (
                source.data.get(from..from + w),
                self.data.get_mut(to..to + w),
            ) else {
                continue;
            };
            if source.alpha {
                for (d, &s) in dst.iter_mut().zip(src) {
                    *d = over(s, *d);
                }
            } else {
                for (d, &s) in dst.iter_mut().zip(src) {
                    *d = s | 0xFF00_0000;
                }
            }
        }
    }
}

/// Premultiplied source-over: `s + d * (1 - alpha(s))`, per channel.
pub fn over(s: u32, d: u32) -> u32 {
    let a = s >> 24;
    if a == 0xFF {
        return s;
    }
    let inv = 255 - a;
    let mut out = 0u32;
    for shift in [0, 8, 16, 24] {
        let sc = (s >> shift) & 0xFF;
        let dc = (d >> shift) & 0xFF;
        // dc * inv / 255, rounded.
        let scaled = (dc * inv + 127) / 255;
        out |= (sc + scaled).min(255) << shift;
    }
    out
}
