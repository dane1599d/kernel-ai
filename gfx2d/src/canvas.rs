//! Pixels and the blits on them. A pixel is `0xAARRGGBB`: premultiplied ARGB, or
//! XRGB with the top byte ignored; blending is premultiplied sRGB source-over,
//! with no linear-light step (Wayland's rule, and `surfabi`'s).

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

    pub fn contains(&self, o: &Rect) -> bool {
        o.is_empty() || self.intersect(o) == *o
    }
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

/// Premultiplied source-over, `src + dst * (1 - src.a)`; the result is opaque
/// when `dst` is.
#[inline(always)]
pub fn over(src: u32, dst: u32) -> u32 {
    let a = src >> 24;
    match a {
        255 => src,
        0 => dst,
        _ => {
            let out_a = a + (((dst >> 24) * (255 - a) + 127) / 255);
            ((src & 0x00FF_FFFF) + scale(dst, 255 - a)) | (out_a << 24)
        }
    }
}

/// `color` (`0x00RRGGBB`) at coverage `a` over `dst`, keeping `dst`'s alpha.
#[inline(always)]
pub fn tint(color: u32, a: u32, dst: u32) -> u32 {
    match a {
        255 => (color & 0x00FF_FFFF) | (dst & 0xFF00_0000),
        0 => dst,
        _ => (scale(color, a) + scale(dst, 255 - a)) | (dst & 0xFF00_0000),
    }
}

/// `color` at each coverage over a row, keeping each pixel's alpha byte. Every
/// product fits 16 bits (255 * 255), so it vectorises on SSE2, which has no
/// 32-bit multiply; `x / 255` is `(x + 1 + (x >> 8)) >> 8`.
pub fn blend_row(dst: &mut [u32], coverage: &[u8], color: u32) {
    let c = [
        (color >> 16) as u16 & 255,
        (color >> 8) as u16 & 255,
        color as u16 & 255,
    ];
    for (p, &a) in dst.iter_mut().zip(coverage) {
        let a = u16::from(a);
        let ia = 255 - a;
        let d = *p;
        let ch = |i: usize, shift: u32| {
            let x = c[i] * a + ((d >> shift) as u16 & 255) * ia;
            u32::from((x + 1 + (x >> 8)) >> 8)
        };
        *p = (d & 0xFF00_0000) | (ch(0, 16) << 16) | (ch(1, 8) << 8) | ch(2, 0);
    }
}

/// Pixels a renderer draws into: `height` rows of `stride_px` u32s, the first
/// `width` shown (a `surfabi` buffer's stride, in bytes, is `4 * stride_px`).
pub struct Canvas<'a> {
    pub pixels: &'a mut [u32],
    pub width: u32,
    pub height: u32,
    pub stride_px: u32,
}

/// An owned picture: a layer, a frame, a pre-rendered shadow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
}

impl Image {
    pub fn new(w: i32, h: i32) -> Image {
        Image {
            w,
            h,
            px: vec![0; (w.max(0) * h.max(0)) as usize],
        }
    }

    pub fn canvas(&mut self) -> Canvas<'_> {
        Canvas {
            width: self.w as u32,
            height: self.h as u32,
            stride_px: self.w as u32,
            pixels: &mut self.px,
        }
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.w, self.h)
    }
}

impl Canvas<'_> {
    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }

    fn check(&self) {
        debug_assert!(
            self.height == 0
                || self.pixels.len() as u64
                    >= u64::from(self.height - 1) * u64::from(self.stride_px)
                        + u64::from(self.width),
            "a canvas past its pixels"
        );
    }

    /// The pixels of row `y` from `x0` to `x1`; the caller clipped them.
    fn row(&mut self, y: i32, x0: i32, x1: i32) -> &mut [u32] {
        let at = y as usize * self.stride_px as usize;
        &mut self.pixels[at + x0 as usize..at + x1 as usize]
    }

    pub fn fill(&mut self, r: Rect, color: u32) {
        self.check();
        let r = r.intersect(&self.bounds());
        for y in r.y..r.bottom() {
            self.row(y, r.x, r.right()).fill(color);
        }
    }

    /// `color` at coverage `a` over `r`.
    pub fn fill_alpha(&mut self, r: Rect, color: u32, a: u32) {
        let r = r.intersect(&self.bounds());
        for y in r.y..r.bottom() {
            for p in self.row(y, r.x, r.right()) {
                *p = tint(color, a, *p);
            }
        }
    }

    /// A 1 px frame just inside `r`.
    pub fn frame(&mut self, r: Rect, color: u32) {
        self.fill(Rect::new(r.x, r.y, r.w, 1), color);
        self.fill(Rect::new(r.x, r.bottom() - 1, r.w, 1), color);
        self.fill(Rect::new(r.x, r.y, 1, r.h), color);
        self.fill(Rect::new(r.right() - 1, r.y, 1, r.h), color);
    }

    /// `src` placed at `at`, as far as it falls in `clip`: copied, or with
    /// `blend` composited source-over.
    pub fn draw(&mut self, src: &Image, at: (i32, i32), clip: Rect, blend: bool) {
        self.check();
        let place = Rect::new(at.0, at.1, src.w, src.h);
        let r = place.intersect(&clip).intersect(&self.bounds());
        if r.is_empty() {
            return;
        }
        let sx = (r.x - at.0) as usize;
        for y in r.y..r.bottom() {
            let s = ((y - at.1) * src.w) as usize + sx;
            let src_row = &src.px[s..s + r.w as usize];
            let dst_row = self.row(y, r.x, r.right());
            if blend {
                for (d, &s) in dst_row.iter_mut().zip(src_row) {
                    *d = over(s, *d);
                }
            } else {
                dst_row.copy_from_slice(src_row);
            }
        }
    }

    /// A window layer at `at`: its rows `radius..h - radius` are opaque and
    /// copied, its top and bottom bands (the rounded corners) blended; a
    /// translucent window (`opaque` false) blends throughout.
    pub fn draw_window(
        &mut self,
        src: &Image,
        at: (i32, i32),
        clip: Rect,
        radius: i32,
        opaque: bool,
    ) {
        if !opaque {
            return self.draw(src, at, clip, true);
        }
        let place = Rect::new(at.0, at.1, src.w, src.h);
        let radius = radius.clamp(0, src.h / 2);
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

impl Image {
    /// Rounds the corners: each corner pixel's alpha becomes its coverage of a
    /// quarter disc of radius `r`, premultiplied.
    pub fn round_corners(&mut self, r: i32) {
        let r = r.clamp(0, self.w.min(self.h) / 2);
        let rf = r as f32;
        for cy in 0..r {
            for cx in 0..r {
                let (dx, dy) = (rf - cx as f32 - 0.5, rf - cy as f32 - 0.5);
                let d = libm::sqrtf(dx * dx + dy * dy);
                let a = ((rf - d + 0.5).clamp(0.0, 1.0) * 255.0) as u32;
                for (x, y) in [
                    (cx, cy),
                    (self.w - 1 - cx, cy),
                    (cx, self.h - 1 - cy),
                    (self.w - 1 - cx, self.h - 1 - cy),
                ] {
                    let p = &mut self.px[(y * self.w + x) as usize];
                    let alpha = (*p >> 24) * a / 255;
                    *p = scale(*p & 0x00FF_FFFF, a) | (alpha << 24);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_over_matches_the_formula() {
        assert_eq!(over(0xFF12_3456, 0xFFAB_CDEF), 0xFF12_3456);
        assert_eq!(over(0x0000_0000, 0xFFAB_CDEF), 0xFFAB_CDEF);
        // Half-opaque black over opaque white is opaque mid grey.
        assert_eq!(over(0x8000_0000, 0xFFFF_FFFF), 0xFF7F_7F7F);
    }

    #[test]
    fn the_16_bit_blend_is_within_one_of_the_exact_one() {
        for a in [0u8, 1, 64, 127, 128, 200, 254, 255] {
            for dst in [0xFF00_0000u32, 0xFFFF_FFFF, 0xFF12_3456, 0x00AB_CDEF] {
                let mut row = [dst];
                blend_row(&mut row, &[a], 0x00FF_9F5A);
                let want = tint(0x00FF_9F5A, u32::from(a), dst);
                assert_eq!(row[0] >> 24, want >> 24, "alpha is kept");
                for shift in [0, 8, 16] {
                    let (g, w) = ((row[0] >> shift) & 255, (want >> shift) & 255);
                    assert!(g.abs_diff(w) <= 1, "a {a} dst {dst:08x}: {g} vs {w}");
                }
            }
        }
    }

    #[test]
    fn a_strided_canvas_draws_only_its_width() {
        let mut px = [7u32; 6 * 3];
        let mut c = Canvas {
            pixels: &mut px,
            width: 4,
            height: 3,
            stride_px: 6,
        };
        c.fill(Rect::new(-5, 1, 100, 1), 1);
        assert_eq!(&px[6..12], &[1, 1, 1, 1, 7, 7]);
        assert!(px[..6].iter().chain(&px[12..]).all(|&p| p == 7));
    }

    #[test]
    fn a_window_copies_its_body_and_blends_its_corners() {
        let mut layer = Image::new(20, 20);
        let bounds = layer.bounds();
        layer.canvas().fill(bounds, 0xFF14_1821);
        layer.round_corners(6);
        assert!(layer.px[0] >> 24 < 255, "the corner is see-through");
        assert_eq!(layer.px[10 * 20 + 10], 0xFF14_1821);
        let mut out = Image::new(30, 30);
        let bounds = out.bounds();
        out.canvas().fill(bounds, 0xFFFF_FFFF);
        out.canvas().draw_window(&layer, (5, 5), bounds, 6, true);
        assert_eq!(out.px[5 * 30 + 5], over(layer.px[0], 0xFFFF_FFFF));
        assert_eq!(out.px[15 * 30 + 15], 0xFF14_1821);
    }

    #[test]
    fn rect_algebra() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);
        assert_eq!(a.intersect(&b), Rect::new(5, 5, 5, 5));
        assert_eq!(a.union(&b), Rect::new(0, 0, 15, 15));
        assert!(a.contains(&Rect::new(2, 2, 3, 3)) && !a.contains(&b));
    }
}
