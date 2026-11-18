//! The one place logical px become physical (ADR 0005 §9). The desk, its
//! layout, its hit tests and its painter all work in logical px; at a scale
//! above 1.0 `desktop` composes the logical frame as usual, and this module
//! resamples the damaged part of it, bilinearly, onto the screen. At scale
//! 1.0 nothing here runs and the frame is copied as it always was.
//!
//! Pure and host-tested: the damage a change needs ([`physical`]) is checked
//! against a full resample.

use crate::geom::Rect;
use crate::scale::ONE;
use alloc::vec::Vec;

/// A weight of 1.0 in the blend.
const UNIT: u32 = 256;
/// Fixed-point fraction bits of a source coordinate.
const FRAC: u32 = 8;

/// The logical size of a `physical` px screen at `scale`: what the desk lays
/// out on. Never under 1.
pub fn logical(physical: i32, scale: u32) -> i32 {
    ((i64::from(physical) * i64::from(ONE)) / i64::from(scale.max(1))).max(1) as i32
}

/// The physical rectangle a change to the logical rectangle `r` can reach:
/// `r` grown by the one logical px a bilinear sample reads past it, then
/// outward to whole physical px, kept on a `width` x `height` screen. `None`
/// when nothing of it is on the screen.
pub fn physical(r: Rect, scale: u32, width: i32, height: i32) -> Option<Rect> {
    let up = |v: i32| i64::from(v) * i64::from(scale);
    let floor = |v: i32| up(v).div_euclid(i64::from(ONE)) as i32;
    let ceil = |v: i32| (up(v) + i64::from(ONE) - 1).div_euclid(i64::from(ONE)) as i32;
    let x0 = floor(r.x - 1).max(0);
    let y0 = floor(r.y - 1).max(0);
    let x1 = ceil(r.right() + 1).min(width);
    let y1 = ceil(r.bottom() + 1).min(height);
    (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
}

/// A logical frame, row by row.
pub struct Source<'a> {
    pub px: &'a [u32],
    pub width: i32,
    pub height: i32,
}

/// A physical screen's rows: `stride` px a row, at most `width` x `height`.
pub struct Target<'a> {
    pub px: &'a mut [u32],
    pub stride: usize,
    pub width: i32,
    pub height: i32,
}

/// One output column's two source columns and the weight of the second.
struct Tap {
    a: usize,
    b: usize,
    w: u32,
}

/// The source coordinate of output px `d`, in 1/256 px, centres aligned:
/// `(d + 0.5) / scale - 0.5`, kept inside `0..=(last * 256)`.
fn at(d: i32, scale: u32, last: i32) -> u32 {
    let centre = (2 * i64::from(d) + 1) * i64::from(ONE) * i64::from(UNIT);
    let v = centre / (2 * i64::from(scale.max(1))) - i64::from(UNIT / 2);
    v.clamp(0, i64::from(last) << FRAC) as u32
}

fn tap(d: i32, scale: u32, len: i32) -> Tap {
    let last = (len - 1).max(0);
    let s = at(d, scale, last);
    let a = (s >> FRAC) as usize;
    Tap {
        a,
        b: (a + 1).min(last as usize),
        w: s & (UNIT - 1),
    }
}

/// `a` blended to `b` by `w` / 256, every channel at once.
fn lerp(a: u32, b: u32, w: u32) -> u32 {
    const LOW: u32 = 0x00FF_00FF;
    let rb = ((a & LOW) * (UNIT - w) + (b & LOW) * w) >> 8 & LOW;
    let ag = (((a >> 8) & LOW) * (UNIT - w) + ((b >> 8) & LOW) * w) & !LOW;
    rb | ag
}

/// Resamples the physical rectangle `rect` of the screen from `src`, the
/// logical frame, at `scale` (120ths). Anything of `rect` off the screen or
/// past what `px` holds is left alone.
pub fn resample(src: &Source<'_>, dst: &mut Target<'_>, scale: u32, rect: Rect) {
    let Some(rect) = rect.intersect(&Rect::new(0, 0, dst.width, dst.height)) else {
        return;
    };
    if src.width < 1 || src.height < 1 || src.px.len() < (src.width * src.height) as usize {
        return;
    }
    let columns: Vec<Tap> = (rect.x..rect.right())
        .map(|x| tap(x, scale, src.width))
        .collect();
    let width = src.width as usize;
    for y in rect.y..rect.bottom() {
        let row = tap(y, scale, src.height);
        let (top, bottom) = (row.a * width, row.b * width);
        let to = y as usize * dst.stride + rect.x as usize;
        let Some(out) = dst.px.get_mut(to..to + columns.len()) else {
            return;
        };
        for (px, c) in out.iter_mut().zip(&columns) {
            let upper = lerp(src.px[top + c.a], src.px[top + c.b], c.w);
            let lower = lerp(src.px[bottom + c.a], src.px[bottom + c.b], c.w);
            *px = lerp(upper, lower, row.w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn frame(w: i32, h: i32) -> Vec<u32> {
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                0xFF00_0000 | ((x as u32 * 37 % 256) << 16) | ((y as u32 * 91 % 256) << 8) | 0x40
            })
            .collect()
    }

    fn all(src: &[u32], w: i32, h: i32, scale: u32) -> (Vec<u32>, i32, i32) {
        let (pw, ph) = (
            (i64::from(w) * i64::from(scale) / 120) as i32,
            (i64::from(h) * i64::from(scale) / 120) as i32,
        );
        let mut out = vec![0u32; (pw * ph) as usize];
        let mut dst = Target {
            px: &mut out,
            stride: pw as usize,
            width: pw,
            height: ph,
        };
        let src = Source {
            px: src,
            width: w,
            height: h,
        };
        resample(&src, &mut dst, scale, Rect::new(0, 0, pw, ph));
        (out, pw, ph)
    }

    #[test]
    fn a_plain_colour_stays_it_at_every_scale() {
        for scale in [150, 180, 210, 240, 360] {
            let src = vec![0xFF12_3456u32; 64 * 40];
            let (out, ..) = all(&src, 64, 40, scale);
            assert!(out.iter().all(|&p| p == 0xFF12_3456), "scale {scale}");
        }
    }

    #[test]
    fn whole_steps_repeat_the_corners_of_the_frame() {
        let src = frame(16, 10);
        let (out, pw, ph) = all(&src, 16, 10, 240);
        assert_eq!(out[0], src[0]);
        assert_eq!(out[(pw - 1) as usize], src[15]);
        assert_eq!(out[((ph - 1) * pw) as usize], src[9 * 16]);
        assert_eq!(*out.last().unwrap(), *src.last().unwrap());
    }

    #[test]
    fn a_blend_of_two_greys_is_between_them() {
        assert_eq!(lerp(0xFF00_0000, 0xFFFF_FFFF, 0), 0xFF00_0000);
        let mid = lerp(0xFF00_0000, 0xFFFF_FFFF, 128);
        for shift in [0, 8, 16] {
            let c = (mid >> shift) & 0xFF;
            assert!((127..=128).contains(&c), "{c}");
        }
    }

    #[test]
    fn the_logical_size_is_the_physical_one_over_the_scale() {
        assert_eq!(logical(2560, 150), 2048);
        assert_eq!(logical(1440, 150), 1152);
        assert_eq!(logical(1920, 120), 1920);
        assert_eq!(logical(3840, 240), 1920);
    }

    /// The property the damage rests on: changing the logical px of a
    /// rectangle and resampling only `physical` of it gives the pixels a full
    /// resample does.
    #[test]
    fn the_physical_damage_of_a_change_covers_every_pixel_it_moves() {
        let (w, h) = (48, 30);
        for scale in [150, 180, 210, 300] {
            let before = frame(w, h);
            let (mut screen, pw, ph) = all(&before, w, h, scale);
            for r in [
                Rect::new(0, 0, 1, 1),
                Rect::new(5, 7, 9, 3),
                Rect::new(w - 3, h - 2, 3, 2),
                Rect::new(20, 0, 1, h),
            ] {
                let mut after = before.clone();
                for y in r.y..r.bottom() {
                    for x in r.x..r.right() {
                        after[(y * w + x) as usize] ^= 0x00FF_FFFF;
                    }
                }
                let (full, ..) = all(&after, w, h, scale);
                let rect = physical(r, scale, pw, ph).unwrap();
                let mut dst = Target {
                    px: &mut screen,
                    stride: pw as usize,
                    width: pw,
                    height: ph,
                };
                let src = Source {
                    px: &after,
                    width: w,
                    height: h,
                };
                resample(&src, &mut dst, scale, rect);
                assert_eq!(screen, full, "scale {scale}, rect {r:?}");
                // Back to the frame the next rectangle starts from.
                let (back, ..) = all(&before, w, h, scale);
                screen = back;
            }
        }
    }

    #[test]
    fn a_rectangle_off_the_screen_has_no_physical_one() {
        assert_eq!(physical(Rect::new(100, 100, 5, 5), 150, 120, 90), None);
        let r = physical(Rect::new(0, 0, 4, 4), 150, 120, 90).unwrap();
        assert_eq!((r.x, r.y), (0, 0));
    }
}
