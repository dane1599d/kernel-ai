//! A client's own pixels on the screen (ADR 0005 §9). A window drawn at the
//! screen's scale is not resampled with the desk: `desktop` fills its content
//! in the logical frame with a key colour (the hole) and writes the window's id
//! into a mask of the same size, in the order it paints, so the mask names the
//! window whose content was painted last at each logical px. After the resample
//! [`overlay`] copies a window's pixels only at px where the frame still shows
//! the hole, the mask holds the window's own id, and the px lies inside both the
//! window's content and its buffer: a window painted over it covers the hole,
//! and a client can never reach another window's px, whatever its buffer's
//! size or the colour it paints. What the painter draws over every window
//! (the agent's outline, the taskbar, the pane, Start, a tooltip, a card and
//! the arrow: [`crate::paint::Painter::covered`]) is marked no one's after
//! it is painted, so no client's px land on it even where a theme painted it
//! in the hole's own colour.
//!
//! All sizes here come from a client or a desk, so the arithmetic is i64 and the
//! slices are indexed with `get`: nothing panics on any value.

use crate::geom::Rect;
use crate::scale::ONE;
use crate::upscale::{ALPHA, Source, Target};
use alloc::vec::Vec;

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

/// Whose px these are: the mask `desktop` kept, the window's id in it (never
/// 0, which is no window) and the window's content in logical px.
pub struct Owner<'a> {
    /// The window id painted last at each logical px, as wide as the frame.
    pub mask: &'a [u32],
    pub id: u32,
    pub content: Rect,
}

/// Where on the screen: the scale, the physical rectangle to touch and the
/// physical top-left of the window's content, where its buffer begins.
#[derive(Clone, Copy)]
pub struct Place {
    pub scale: u32,
    pub rect: Rect,
    pub origin: (i32, i32),
}

/// Writes `id` into the px of `rect` that `mask`, `width` x `height` px, has.
pub fn mark(mask: &mut [u32], width: i32, height: i32, rect: Rect, id: u32) {
    let (w, h) = (i64::from(width.max(0)), i64::from(height.max(0)));
    let x0 = i64::from(rect.x).clamp(0, w);
    let y0 = i64::from(rect.y).clamp(0, h);
    let x1 = (i64::from(rect.x) + i64::from(rect.w.max(0))).clamp(0, w);
    let y1 = (i64::from(rect.y) + i64::from(rect.h.max(0))).clamp(0, h);
    for y in y0..y1 {
        let from = (y * w + x0) as usize;
        if let Some(row) = mask.get_mut(from..from + (x1 - x0) as usize) {
            row.fill(id);
        }
    }
}

/// `v` logical px as physical, rounded as a client rounds its own.
fn physical(v: i64, scale: u32) -> i64 {
    (v * i64::from(scale) + i64::from(ONE) / 2).div_euclid(i64::from(ONE))
}

/// Puts a client's own pixels on the screen: the physical `rect`, cut to the
/// owner's content, to the buffer laid at `origin` (the physical top-left of
/// the content) and to the screen, and only at px whose logical px in `frame`
/// are exactly `hole` and whose place in `owner.mask` is the owner's id.
pub fn overlay(
    frame: &Source<'_>,
    hole: u32,
    owner: &Owner<'_>,
    place: &Place,
    dst: &mut Target<'_>,
    buffer: &Buffer<'_>,
) {
    let Place {
        scale,
        rect,
        origin,
    } = *place;
    let (fw, fh) = (i64::from(frame.width), i64::from(frame.height));
    let size = fw * fh;
    if fw < 1 || fh < 1 || (frame.px.len() as i64) < size || (owner.mask.len() as i64) < size {
        return;
    }
    let c = owner.content;
    let right = |at: i32, len: i32| i64::from(at) + i64::from(len.max(0));
    // The px that may change: the rect asked for, the screen, the content and
    // the buffer, each as the smallest and the largest it allows.
    let lo = |a: i64, b: i64, c: i64, d: i64| a.max(b).max(c).max(d);
    let hi = |a: i64, b: i64, c: i64, d: i64| a.min(b).min(c).min(d);
    let x0 = lo(
        i64::from(rect.x),
        0,
        physical(i64::from(c.x), scale),
        i64::from(origin.0),
    );
    let y0 = lo(
        i64::from(rect.y),
        0,
        physical(i64::from(c.y), scale),
        i64::from(origin.1),
    );
    let x1 = hi(
        right(rect.x, rect.w),
        i64::from(dst.width),
        physical(right(c.x, c.w), scale),
        right(origin.0, buffer.width),
    );
    let y1 = hi(
        right(rect.y, rect.h),
        i64::from(dst.height),
        physical(right(c.y, c.h), scale),
        right(origin.1, buffer.height),
    );
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    // The logical px a physical one is the centre of.
    let at = |d: i64, len: i64| {
        let s = i64::from(scale.max(1));
        ((2 * d + 1) * i64::from(ONE) / (2 * s)).min(len - 1) as usize
    };
    let columns: Vec<usize> = (x0..x1).map(|x| at(x, fw)).collect();
    for y in y0..y1 {
        let row = at(y, fh) * fw as usize;
        let (Some(held), Some(owned)) = (frame.px.get(row..), owner.mask.get(row..)) else {
            continue;
        };
        let from = (y - i64::from(origin.1))
            .checked_mul(i64::try_from(buffer.stride).unwrap_or(i64::MAX))
            .and_then(|r| r.checked_add(x0 - i64::from(origin.0)))
            .and_then(|f| usize::try_from(f).ok());
        let to = usize::try_from(y)
            .ok()
            .and_then(|y| y.checked_mul(dst.stride))
            .and_then(|r| r.checked_add(x0 as usize));
        let (Some(from), Some(to)) = (from, to) else {
            continue;
        };
        let n = columns.len();
        let (Some(src), Some(out)) = (
            from.checked_add(n).and_then(|end| buffer.px.get(from..end)),
            to.checked_add(n).and_then(|end| dst.px.get_mut(to..end)),
        ) else {
            continue;
        };
        for ((px, &s), &c) in out.iter_mut().zip(src).zip(&columns) {
            if held[c] == hole && owned[c] == owner.id {
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
    use crate::upscale::resample;
    use alloc::vec;

    const BLACK: u32 = 0xFF << 24;
    const BASE: u32 = BLACK | (0x20 << 16) | (0x30 << 8) | 0x40;
    /// The key colour, and what two windows draw.
    const HOLE: u32 = BLACK | (1 << 16) | (2 << 8) | 3;
    const MARK_A: u32 = BLACK | (0x77 << 8) | 0x11;
    const MARK_B: u32 = BLACK | (0x55 << 16) | 0x22;
    const W: i32 = 40;
    const H: i32 = 20;

    /// A frame of base colour, its mask empty, and a screen as the same frame
    /// resampled at 1x (the same px).
    struct Desk {
        frame: Vec<u32>,
        mask: Vec<u32>,
        screen: Vec<u32>,
    }

    impl Desk {
        fn new() -> Self {
            let frame = vec![BASE; (W * H) as usize];
            Desk {
                mask: vec![0; (W * H) as usize],
                screen: frame.clone(),
                frame,
            }
        }

        /// What `desktop` does for a sharp window: a hole, owned.
        fn sharp(&mut self, content: Rect, id: u32) {
            for y in content.y..content.bottom() {
                for x in content.x..content.right() {
                    self.frame[(y * W + x) as usize] = HOLE;
                }
            }
            mark(&mut self.mask, W, H, content, id);
        }

        /// A window that is not sharp: its own px, whatever they are, and no owner.
        fn plain(&mut self, content: Rect, colour: u32) {
            for y in content.y..content.bottom() {
                for x in content.x..content.right() {
                    self.frame[(y * W + x) as usize] = colour;
                }
            }
            mark(&mut self.mask, W, H, content, 0);
        }

        /// The screen from the frame, then the overlay of one window.
        fn show(&mut self, id: u32, content: Rect, buffer: &Buffer, origin: (i32, i32)) {
            let src = Source {
                px: &self.frame,
                width: W,
                height: H,
            };
            let mut dst = Target {
                px: &mut self.screen,
                stride: W as usize,
                width: W,
                height: H,
            };
            let owner = Owner {
                mask: &self.mask,
                id,
                content,
            };
            let place = Place {
                scale: 120,
                rect: Rect::new(0, 0, W, H),
                origin,
            };
            overlay(&src, HOLE, &owner, &place, &mut dst, buffer);
        }

        fn base(&mut self) {
            let src = Source {
                px: &self.frame,
                width: W,
                height: H,
            };
            let mut dst = Target {
                px: &mut self.screen,
                stride: W as usize,
                width: W,
                height: H,
            };
            resample(&src, &mut dst, 120, Rect::new(0, 0, W, H));
        }

        fn at(&self, x: i32, y: i32) -> u32 {
            self.screen[(y * W + x) as usize]
        }
    }

    fn buffer(px: &[u32], width: i32, height: i32) -> Buffer<'_> {
        Buffer {
            px,
            width,
            height,
            stride: width.max(0) as usize,
            alpha: false,
        }
    }

    #[test]
    fn a_big_buffer_beside_another_window_stays_in_its_own_content() {
        let (a, b) = (Rect::new(0, 0, 10, 10), Rect::new(10, 0, 10, 10));
        let big = vec![MARK_A; (W * H) as usize];
        let small = vec![MARK_B; 100];
        // Either order, as the desk's z-order may put either window first.
        for a_first in [true, false] {
            let mut desk = Desk::new();
            desk.sharp(a, 1);
            desk.sharp(b, 2);
            desk.base();
            let (big, small) = (buffer(&big, W, H), buffer(&small, 10, 10));
            match a_first {
                true => {
                    desk.show(1, a, &big, (0, 0));
                    desk.show(2, b, &small, (10, 0));
                }
                false => {
                    desk.show(2, b, &small, (10, 0));
                    desk.show(1, a, &big, (0, 0));
                }
            }
            for y in 0..H {
                for x in 0..W {
                    let want = match (x, y) {
                        (0..10, 0..10) => MARK_A,
                        (10..20, 0..10) => MARK_B,
                        _ => BASE,
                    };
                    assert_eq!(desk.at(x, y), want, "{x},{y} a first: {a_first}");
                }
            }
        }
    }

    #[test]
    fn a_small_buffer_over_a_hostile_lower_window_leaves_it_alone() {
        let mut desk = Desk::new();
        // A lower window that is not sharp and paints the key colour itself.
        let lower = Rect::new(0, 0, 30, 15);
        desk.plain(lower, HOLE);
        // A sharp window above it, with a buffer smaller than its content.
        let upper = Rect::new(5, 2, 10, 8);
        desk.sharp(upper, 3);
        desk.base();
        let px = vec![MARK_A; 4 * 3];
        desk.show(3, upper, &buffer(&px, 4, 3), (5, 2));
        for y in 0..H {
            for x in 0..W {
                let want = match (x, y) {
                    (5..9, 2..5) => MARK_A,
                    (0..30, 0..15) => HOLE,
                    _ => BASE,
                };
                assert_eq!(desk.at(x, y), want, "{x},{y}");
            }
        }
        // And the hostile window's key colour is nothing a sharp window's
        // buffer reaches, even though it asks for all of the screen.
        let all = vec![MARK_B; (W * H) as usize];
        desk.show(3, upper, &buffer(&all, W, H), (0, 0));
        assert_eq!(desk.at(2, 2), HOLE, "the hostile window's px");
        assert_eq!(desk.at(20, 10), HOLE);
    }

    #[test]
    fn covered_px_stay_covered_and_a_stale_owner_is_no_owner() {
        let mut desk = Desk::new();
        let content = Rect::new(0, 0, 20, 10);
        desk.sharp(content, 1);
        // The taskbar-like strip over its right half is painted after.
        for y in 0..10 {
            for x in 10..20 {
                desk.frame[(y * W + x) as usize] = MARK_B;
            }
        }
        desk.base();
        let px = vec![MARK_A; (20 * 10) as usize];
        desk.show(1, content, &buffer(&px, 20, 10), (0, 0));
        assert_eq!(desk.at(5, 5), MARK_A);
        assert_eq!(desk.at(15, 5), MARK_B);
        // The same px for a window that is not the owner: nothing.
        let mut desk = Desk::new();
        desk.sharp(content, 1);
        desk.base();
        desk.show(2, content, &buffer(&px, 20, 10), (0, 0));
        assert_eq!(desk.at(5, 5), HOLE);
    }

    #[test]
    fn no_size_a_client_or_desk_gives_panics_or_reaches_past_its_content() {
        let content = Rect::new(2, 2, 6, 6);
        let px = vec![MARK_A; 16];
        let sizes = [
            (0, i32::MAX),
            (i32::MAX, 0),
            (i32::MAX, i32::MAX),
            (i32::MIN, i32::MIN),
            (-1, 4),
            (4, -1),
            (0, 0),
        ];
        let origins = [(0, 0), (2, 2), (i32::MAX, i32::MAX), (i32::MIN, i32::MIN)];
        let contents = [
            content,
            Rect::new(i32::MAX, i32::MAX, i32::MAX, i32::MAX),
            Rect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX),
            Rect::new(0, 0, i32::MAX, i32::MAX),
        ];
        for (width, height) in sizes {
            for origin in origins {
                for c in contents {
                    let mut desk = Desk::new();
                    desk.sharp(content, 1);
                    desk.base();
                    let before = desk.screen.clone();
                    let b = Buffer {
                        px: &px,
                        width,
                        height,
                        stride: usize::MAX,
                        alpha: false,
                    };
                    desk.show(1, c, &b, origin);
                    // Whatever it changes is the owner's own content, in its px.
                    for (i, (&now, &was)) in desk.screen.iter().zip(&before).enumerate() {
                        let (x, y) = (i as i32 % W, i as i32 / W);
                        if now != was {
                            let ok = content.contains(crate::geom::Point { x, y });
                            assert!(
                                ok && now == MARK_A,
                                "{x},{y}: {width}x{height} at {origin:?}"
                            );
                        }
                    }
                }
            }
        }
        // A frame or mask shorter than its size is no frame.
        let mut desk = Desk::new();
        desk.sharp(content, 1);
        desk.base();
        desk.mask.truncate(3);
        let before = desk.screen.clone();
        desk.show(1, content, &buffer(&px, 4, 4), (2, 2));
        assert_eq!(desk.screen, before);
    }

    #[test]
    fn marking_a_rectangle_off_the_mask_marks_what_is_on_it() {
        let mut mask = vec![0u32; 12];
        mark(&mut mask, 4, 3, Rect::new(2, 1, 99, 99), 7);
        assert_eq!(mask, [0, 0, 0, 0, 0, 0, 7, 7, 0, 0, 7, 7]);
        mark(
            &mut mask,
            4,
            3,
            Rect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX),
            9,
        );
        assert_eq!(mask.iter().filter(|&&m| m == 9).count(), 0);
        mark(&mut mask, 4, 3, Rect::new(-2, 0, 3, 1), 5);
        assert_eq!(&mask[..4], &[5, 0, 0, 0]);
        mark(&mut mask, i32::MAX, i32::MAX, Rect::new(0, 0, 1, 1), 1);
        assert_eq!(mask.len(), 12);
    }

    #[test]
    fn at_a_fractional_scale_only_the_owners_px_are_replaced() {
        // 1.25x on a 60x37 screen, the logical frame 48x30 with a window at
        // (8, 8, 16, 8): the physical content is (10, 10, 20, 10).
        let (w, h) = (48, 30);
        let mut frame = vec![BASE; (w * h) as usize];
        let mut mask = vec![0u32; (w * h) as usize];
        let content = Rect::new(8, 8, 16, 8);
        for y in 8..16 {
            for x in 8..24 {
                frame[(y * w + x) as usize] = HOLE;
            }
        }
        mark(&mut mask, w, h, content, 4);
        let src = Source {
            px: &frame,
            width: w,
            height: h,
        };
        let mut screen = vec![0u32; 60 * 37];
        let mut dst = Target {
            px: &mut screen,
            stride: 60,
            width: 60,
            height: 37,
        };
        resample(&src, &mut dst, 150, Rect::new(0, 0, 60, 37));
        // A buffer far larger than the window, laid at its origin.
        let big = vec![MARK_A; 60 * 37];
        let owner = Owner {
            mask: &mask,
            id: 4,
            content,
        };
        let place = Place {
            scale: 150,
            rect: Rect::new(0, 0, 60, 37),
            origin: (10, 10),
        };
        overlay(&src, HOLE, &owner, &place, &mut dst, &buffer(&big, 60, 37));
        for y in 0..37 {
            for x in 0..60 {
                let inside = (10..30).contains(&x) && (10..20).contains(&y);
                let got = screen[(y * 60 + x) as usize];
                assert_eq!(got == MARK_A, inside, "{x},{y}");
            }
        }
    }
}
