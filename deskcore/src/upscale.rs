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

/// The alpha byte: a screen's pixels are opaque, whatever the frame held
/// (a hole in it has none).
const ALPHA: u32 = 0xFF << 24;
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

/// `a` blended to `b` by `w` / 256, every channel at once. Two channels a lane
/// in 16 bits: each sum is at most 255 * 256, so nothing carries into the next
/// lane; wrapping ops say so, and keep the loops free of overflow checks (the
/// dev profile's) so they vectorise.
fn lerp(a: u32, b: u32, w: u32) -> u32 {
    const LOW: u32 = u32::MAX / 257;
    let (inv, w) = (UNIT - w, w);
    let rb = ((a & LOW)
        .wrapping_mul(inv)
        .wrapping_add((b & LOW).wrapping_mul(w))
        >> 8)
        & LOW;
    let ag = ((a >> 8) & LOW)
        .wrapping_mul(inv)
        .wrapping_add(((b >> 8) & LOW).wrapping_mul(w))
        & !LOW;
    rb | ag
}

/// Resamples the physical rectangle `rect` of the screen from `src`, the
/// logical frame, at `scale` (120ths). Anything of `rect` off the screen or
/// past what `px` holds is left alone.
///
/// Two passes, so each source row is blended across once and each output row
/// is one blend of two such rows: the horizontal pass keeps the row above and
/// the row below the output row's place, and moves down a source row at a time.
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
    let across = |row: usize, out: &mut Vec<u32>| {
        let from = &src.px[row * width..(row + 1) * width];
        out.clear();
        out.extend(columns.iter().map(|c| lerp(from[c.a], from[c.b], c.w)));
    };
    let (mut top, mut bottom) = (Vec::with_capacity(columns.len()), Vec::new());
    let mut held: Option<usize> = None;
    for y in rect.y..rect.bottom() {
        let row = tap(y, scale, src.height);
        // The rows held are `row.a` and the one after: a step down of one row
        // reuses the lower as the upper, any other starts over.
        match held {
            Some(at) if at == row.a => {}
            Some(at) if at + 1 == row.a => {
                core::mem::swap(&mut top, &mut bottom);
                across((row.a + 1).min(src.height as usize - 1), &mut bottom);
            }
            _ => {
                across(row.a, &mut top);
                across((row.a + 1).min(src.height as usize - 1), &mut bottom);
            }
        }
        held = Some(row.a);
        let to = y as usize * dst.stride + rect.x as usize;
        let Some(out) = dst.px.get_mut(to..to + columns.len()) else {
            return;
        };
        match row.w {
            0 => {
                for (px, &up) in out.iter_mut().zip(&top) {
                    *px = up | ALPHA;
                }
            }
            w => {
                for ((px, &up), &down) in out.iter_mut().zip(&top).zip(&bottom) {
                    *px = lerp(up, down, w) | ALPHA;
                }
            }
        }
    }
}

/// A client's buffer, drawn at the scale the desk shows it at.
pub struct Buffer<'a> {
    pub px: &'a [u32],
    pub width: i32,
    pub height: i32,
    /// Pixels a row.
    pub stride: usize,
    /// Whether the top byte is premultiplied alpha, or ignored.
    pub alpha: bool,
}

/// Puts a client's own pixels on the screen where its window shows through
/// the logical `frame`: the physical `rect`, cut to the buffer laid at
/// `origin` (the physical top-left of the window's content), and only at
/// px whose logical px in `frame` are exactly `hole`, the value `desktop`
/// filled the content with and nothing painted over. A window behind another,
/// the taskbar, a card or the cursor has covered its hole, so what is on top
/// is left on top, and the client's pixels are never resampled.
pub fn overlay(
    frame: &Source<'_>,
    hole: u32,
    scale: u32,
    dst: &mut Target<'_>,
    rect: Rect,
    origin: (i32, i32),
    buffer: &Buffer<'_>,
) {
    let content = Rect::new(origin.0, origin.1, buffer.width, buffer.height);
    let Some(rect) = rect
        .intersect(&content)
        .and_then(|r| r.intersect(&Rect::new(0, 0, dst.width, dst.height)))
    else {
        return;
    };
    if frame.width < 1 || frame.height < 1 || frame.px.len() < (frame.width * frame.height) as usize
    {
        return;
    }
    // The logical px a physical one is the centre of.
    let at = |d: i32, len: i32| {
        let s = i64::from(scale.max(1));
        ((2 * i64::from(d) + 1) * i64::from(ONE) / (2 * s)).min(i64::from(len) - 1) as usize
    };
    let columns: Vec<usize> = (rect.x..rect.right()).map(|x| at(x, frame.width)).collect();
    for y in rect.y..rect.bottom() {
        let held = &frame.px[at(y, frame.height) * frame.width as usize..];
        let from = (y - origin.1) as usize * buffer.stride + (rect.x - origin.0) as usize;
        let to = y as usize * dst.stride + rect.x as usize;
        let (Some(src), Some(out)) = (
            buffer.px.get(from..from + columns.len()),
            dst.px.get_mut(to..to + columns.len()),
        ) else {
            continue;
        };
        for ((px, &s), &c) in out.iter_mut().zip(src).zip(&columns) {
            if held[c] == hole {
                *px = match buffer.alpha {
                    true => crate::compose::over(s, *px) | ALPHA,
                    false => s | ALPHA,
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Opaque black, and a colour: built from channels, not written as a literal.
    const BLACK: u32 = 0xFF << 24;
    const WHITE: u32 = u32::MAX;
    const TEAL: u32 = BLACK | (0x12 << 16) | (0x34 << 8) | 0x56;

    fn frame(w: i32, h: i32) -> Vec<u32> {
        (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                BLACK | ((x as u32 * 37 % 256) << 16) | ((y as u32 * 91 % 256) << 8) | 0x40
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
            let src = vec![TEAL; 64 * 40];
            let (out, ..) = all(&src, 64, 40, scale);
            assert!(out.iter().all(|&p| p == TEAL), "scale {scale}");
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
        assert_eq!(lerp(BLACK, WHITE, 0), BLACK);
        let mid = lerp(BLACK, WHITE, 128);
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
                        after[(y * w + x) as usize] ^= WHITE >> 8;
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

    /// A logical frame with a hole (a client's window, not yet drawn) at
    /// (8, 8, 16, 8), 1.25x on a 60x37 screen.
    const HOLE: u32 = (0x12 << 16) | 0x34;
    const MARK: u32 = BLACK | (0x77 << 8) | 0x11;

    fn holed() -> (Vec<u32>, i32, i32) {
        let (w, h) = (48, 30);
        let mut frame = frame(w, h);
        for y in 8..16 {
            for x in 8..24 {
                frame[(y * w + x) as usize] = HOLE;
            }
        }
        (frame, w, h)
    }

    fn overlaid(frame: &[u32], w: i32, h: i32, screen: &mut Vec<u32>, buffer: &Buffer) {
        let (pw, ph) = (60, 37);
        let src = Source {
            px: frame,
            width: w,
            height: h,
        };
        let mut dst = Target {
            px: screen,
            stride: pw as usize,
            width: pw,
            height: ph,
        };
        resample(&src, &mut dst, 150, Rect::new(0, 0, pw, ph));
        overlay(
            &src,
            HOLE,
            150,
            &mut dst,
            Rect::new(0, 0, pw, ph),
            (10, 10),
            buffer,
        );
    }

    #[test]
    fn a_clients_pixels_replace_its_hole_and_nothing_else() {
        let (frame, w, h) = holed();
        let pixels = vec![MARK; 20 * 10];
        let buffer = Buffer {
            px: &pixels,
            width: 20,
            height: 10,
            stride: 20,
            alpha: false,
        };
        let mut screen = vec![0; 60 * 37];
        overlaid(&frame, w, h, &mut screen, &buffer);
        let (plain, ..) = all(&frame, w, h, 150);
        for y in 0..37 {
            for x in 0..60 {
                let i = (y * 60 + x) as usize;
                let inside = (10..30).contains(&x) && (10..20).contains(&y);
                match inside {
                    true => assert_eq!(screen[i], MARK, "{x},{y}"),
                    false => assert_eq!(screen[i], plain[i] | ALPHA, "{x},{y}"),
                }
            }
        }
    }

    #[test]
    fn what_covers_the_hole_stays_on_top_of_the_client() {
        let (mut frame, w, h) = holed();
        // A taskbar-like strip over the hole's right half.
        for y in 8..16 {
            for x in 16..24 {
                frame[(y * w + x) as usize] = TEAL;
            }
        }
        let pixels = vec![MARK; 20 * 10];
        let buffer = Buffer {
            px: &pixels,
            width: 20,
            height: 10,
            stride: 20,
            alpha: false,
        };
        let mut screen = vec![0; 60 * 37];
        overlaid(&frame, w, h, &mut screen, &buffer);
        assert_eq!(screen[(12 * 60 + 12) as usize], MARK);
        assert_eq!(screen[(12 * 60 + 25) as usize], TEAL);
    }

    #[test]
    fn a_buffer_smaller_than_the_hole_shows_the_rest_of_the_hole_resampled() {
        let (frame, w, h) = holed();
        let pixels = vec![MARK; 10 * 10];
        let buffer = Buffer {
            px: &pixels,
            width: 10,
            height: 10,
            stride: 10,
            alpha: false,
        };
        let mut screen = vec![0; 60 * 37];
        overlaid(&frame, w, h, &mut screen, &buffer);
        assert_eq!(screen[(12 * 60 + 12) as usize], MARK);
        assert_eq!(screen[(12 * 60 + 25) as usize], HOLE | ALPHA);
    }

    #[test]
    fn a_translucent_buffer_blends_over_what_is_there() {
        let (frame, w, h) = holed();
        let half = (0x80 << 24) | (0x40 << 16);
        let pixels = vec![half; 20 * 10];
        let buffer = Buffer {
            px: &pixels,
            width: 20,
            height: 10,
            stride: 20,
            alpha: true,
        };
        let mut screen = vec![0; 60 * 37];
        overlaid(&frame, w, h, &mut screen, &buffer);
        let got = screen[(12 * 60 + 12) as usize];
        assert_eq!(got >> 24, 0xFF);
        assert_eq!(got, crate::compose::over(half, HOLE | ALPHA) | ALPHA);
    }

    /// How long a whole-screen resample takes, 2048x1152 to 2560x1440 (1440p at
    /// 1.25), best of five. A probe for the host, not a gate: `cargo test -p
    /// deskcore --release upscale_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "a timing probe"]
    fn upscale_cost() {
        let (w, h, scale) = (2048, 1152, 150);
        let src = frame(w, h);
        let (pw, ph) = (2560, 1440);
        let mut out = vec![0u32; (pw * ph) as usize];
        let best = (0..5)
            .map(|_| {
                let started = std::time::Instant::now();
                let mut dst = Target {
                    px: &mut out,
                    stride: pw as usize,
                    width: pw,
                    height: ph,
                };
                let src = Source {
                    px: &src,
                    width: w,
                    height: h,
                };
                resample(&src, &mut dst, scale, Rect::new(0, 0, pw, ph));
                started.elapsed()
            })
            .min()
            .unwrap();
        std::println!("upscale {w}x{h} -> {pw}x{ph}: {best:?}");
    }
}
