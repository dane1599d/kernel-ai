//! An XRGB8888 frame in RAM, rectangles, and the blits compositing needs:
//! opaque copies, premultiplied source-over, and a nine-slice shadow.

use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect { x, y, w, h }
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn intersect(&self, o: &Rect) -> Rect {
        let (x, y) = (self.x.max(o.x), self.y.max(o.y));
        let (r, b) = (self.right().min(o.right()), self.bottom().min(o.bottom()));
        Rect::new(x, y, (r - x).max(0), (b - y).max(0))
    }

    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let (x, y) = (self.x.min(o.x), self.y.min(o.y));
        let (r, b) = (self.right().max(o.right()), self.bottom().max(o.bottom()));
        Rect::new(x, y, r - x, b - y)
    }

    pub fn grow(&self, by: i32) -> Rect {
        Rect::new(self.x - by, self.y - by, self.w + 2 * by, self.h + 2 * by)
    }

    pub fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    pub fn area(&self) -> i64 {
        if self.is_empty() {
            0
        } else {
            i64::from(self.w) * i64::from(self.h)
        }
    }
}

/// `width` x `height` pixels, `0x00RRGGBB` (or premultiplied `0xAARRGGBB` for a
/// layer), rows packed.
#[derive(Clone)]
pub struct Frame {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
}

/// `c` with each channel times `a / 255`, rounded.
#[inline(always)]
fn scale(c: u32, a: u32) -> u32 {
    let rb = (c & 0x00FF_00FF) * a + 0x0080_0080;
    let g = (c & 0x0000_FF00) * a + 0x0000_8000;
    let rb = ((rb + ((rb >> 8) & 0x00FF_00FF)) >> 8) & 0x00FF_00FF;
    let g = ((g + ((g >> 8) & 0x0000_FF00)) >> 8) & 0x0000_FF00;
    rb | g
}

/// Premultiplied source-over: `src + dst * (1 - src.a)`.
#[inline(always)]
pub fn over(src: u32, dst: u32) -> u32 {
    let a = src >> 24;
    match a {
        255 => src & 0x00FF_FFFF,
        0 => dst,
        _ => (src & 0x00FF_FFFF) + scale(dst, 255 - a),
    }
}

/// `color` at coverage `a` (0-255) over `dst`.
#[inline(always)]
pub fn tint(color: u32, a: u32, dst: u32) -> u32 {
    match a {
        255 => color,
        0 => dst,
        _ => scale(color, a) + scale(dst, 255 - a),
    }
}

impl Frame {
    pub fn new(w: i32, h: i32) -> Frame {
        Frame {
            w,
            h,
            px: vec![0; (w.max(0) * h.max(0)) as usize],
        }
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.w, self.h)
    }

    pub fn fill(&mut self, r: Rect, color: u32) {
        let r = r.intersect(&self.bounds());
        for y in r.y..r.bottom() {
            let row = (y * self.w) as usize;
            self.px[row + r.x as usize..row + r.right() as usize].fill(color);
        }
    }

    /// Blends `color` at coverage `a` over `r`.
    pub fn fill_alpha(&mut self, r: Rect, color: u32, a: u32) {
        let r = r.intersect(&self.bounds());
        for y in r.y..r.bottom() {
            let row = (y * self.w) as usize;
            for p in &mut self.px[row + r.x as usize..row + r.right() as usize] {
                *p = tint(color, a, *p);
            }
        }
    }

    /// Copies `src`'s pixels whose place `at + p` falls in `clip`; `blend` treats
    /// them as premultiplied ARGB, otherwise they are opaque.
    pub fn draw(&mut self, src: &Frame, at: (i32, i32), clip: Rect, blend: bool) {
        let place = Rect::new(at.0, at.1, src.w, src.h);
        let r = place.intersect(&clip).intersect(&self.bounds());
        if r.is_empty() {
            return;
        }
        let sx = (r.x - at.0) as usize;
        for y in r.y..r.bottom() {
            let s = ((y - at.1) * src.w) as usize + sx;
            let d = (y * self.w + r.x) as usize;
            let (src_row, dst_row) = (
                &src.px[s..s + r.w as usize],
                &mut self.px[d..d + r.w as usize],
            );
            if blend {
                for (d, &s) in dst_row.iter_mut().zip(src_row) {
                    *d = over(s, *d);
                }
            } else {
                dst_row.copy_from_slice(src_row);
            }
        }
    }
}

impl Frame {
    /// A window layer at `at`: rows `radius..h - radius` are opaque and copied,
    /// the top and bottom bands (the rounded corners) are blended. `opaque`
    /// false blends it all (a translucent window).
    pub fn draw_window(
        &mut self,
        src: &Frame,
        at: (i32, i32),
        clip: Rect,
        radius: i32,
        opaque: bool,
    ) {
        if !opaque {
            return self.draw(src, at, clip, true);
        }
        let place = Rect::new(at.0, at.1, src.w, src.h);
        let bands = [
            (Rect::new(place.x, place.y, place.w, radius), true),
            (
                Rect::new(place.x, place.y + radius, place.w, place.h - 2 * radius),
                false,
            ),
            (
                Rect::new(place.x, place.bottom() - radius, place.w, radius),
                true,
            ),
        ];
        for (band, blend) in bands {
            self.draw(src, at, band.intersect(&clip), blend);
        }
    }
}

/// A shadow, pre-rendered once: a blurred rounded box's alpha, `3 * m` square,
/// drawn as nine slices around a window so a moving shadow costs a blit.
pub struct Shadow {
    pub m: i32,
    pub alpha: Vec<u8>,
    pub color: u32,
}

impl Shadow {
    /// A box blur three times over a square of side `m`, `m` in from each edge.
    pub fn new(m: i32, strength: u8, color: u32) -> Shadow {
        let side = 3 * m;
        let mut a: Vec<u32> = vec![0; (side * side) as usize];
        for y in m..2 * m {
            for x in m..2 * m {
                a[(y * side + x) as usize] = u32::from(strength);
            }
        }
        let radius = (m / 3).max(1);
        for _ in 0..3 {
            a = blur(&a, side, radius, true);
            a = blur(&a, side, radius, false);
        }
        Shadow {
            m,
            alpha: a.iter().map(|&v| v.min(255) as u8).collect(),
            color,
        }
    }

    /// The shadow of a window at `r`, as far as it falls in `clip`.
    pub fn draw(&self, f: &mut Frame, r: Rect, clip: Rect) {
        let (m, side) = (self.m, 3 * self.m);
        let outer = r.grow(m).intersect(&clip).intersect(&f.bounds());
        for y in outer.y..outer.bottom() {
            // Which source row: the top band, the middle, or the bottom band.
            let sy = if y < r.y + m {
                y - (r.y - m)
            } else if y >= r.bottom() - m {
                y - (r.bottom() - m) + 2 * m
            } else {
                m + m / 2
            };
            let row = (y * f.w) as usize;
            for x in outer.x..outer.right() {
                if x >= r.x && x < r.right() && y >= r.y && y < r.bottom() {
                    continue;
                }
                let sx = if x < r.x + m {
                    x - (r.x - m)
                } else if x >= r.right() - m {
                    x - (r.right() - m) + 2 * m
                } else {
                    m + m / 2
                };
                let a = u32::from(
                    self.alpha[(sy.clamp(0, side - 1) * side + sx.clamp(0, side - 1)) as usize],
                );
                let p = &mut f.px[row + x as usize];
                *p = tint(self.color, a, *p);
            }
        }
    }
}

fn blur(a: &[u32], side: i32, radius: i32, horizontal: bool) -> Vec<u32> {
    let mut out = vec![0u32; a.len()];
    let n = (2 * radius + 1) as u32;
    for line in 0..side {
        let at = |i: i32| -> u32 {
            if i < 0 || i >= side {
                return 0;
            }
            let idx = if horizontal {
                line * side + i
            } else {
                i * side + line
            };
            a[idx as usize]
        };
        let mut sum: u32 = (-radius..=radius).map(at).sum();
        for i in 0..side {
            let idx = if horizontal {
                line * side + i
            } else {
                i * side + line
            };
            out[idx as usize] = sum / n;
            sum = sum + at(i + radius + 1) - at(i - radius);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_over_matches_the_formula() {
        assert_eq!(over(0xFF12_3456, 0x00AB_CDEF), 0x0012_3456);
        assert_eq!(over(0x0000_0000, 0x00AB_CDEF), 0x00AB_CDEF);
        // Half-opaque black over white is mid grey.
        assert_eq!(over(0x8000_0000, 0x00FF_FFFF), 0x007F_7F7F);
    }

    #[test]
    fn rect_algebra() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        assert_eq!(a.intersect(&b), Rect::new(5, 5, 5, 5));
        assert_eq!(a.union(&b), Rect::new(0, 0, 15, 15));
        assert!(a.intersect(&Rect::new(20, 20, 1, 1)).is_empty());
    }
}
