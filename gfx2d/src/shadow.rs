//! Shadows, pre-rendered once as nine-slice alpha (ADR 0005 §5): a blurred
//! rounded box `3m` square, its corners drawn as they are and its edges
//! stretched, so a moving shadow costs a blit. `desktop` keeps two layers for
//! the focused window.

use crate::canvas::{Canvas, Rect, tint};
use alloc::vec;
use alloc::vec::Vec;

pub struct Shadow {
    /// How far it reaches outside the window.
    pub m: i32,
    alpha: Vec<u8>,
    color: u32,
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

impl Shadow {
    /// A shadow reaching `m` px, at most `strength` (0-255) of `color`: a box blur
    /// three times over a square inset by `m`.
    pub fn new(m: i32, strength: u8, color: u32) -> Shadow {
        let m = m.max(1);
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

    /// The shadow of a window at `r`, outside it, as far as it falls in `clip`.
    pub fn draw(&self, c: &mut Canvas<'_>, r: Rect, clip: Rect) {
        let stride = c.stride_px as usize;
        let bounds = c.bounds();
        self.each(r, clip.intersect(&bounds), |x, y, a| {
            let p = &mut c.pixels[y as usize * stride + x as usize];
            *p = tint(self.color, a, *p);
        });
    }

    /// [`Shadow::draw`], and in the same pass what it leaves of the light at
    /// each px, for a shadow of black: `shade`, `c.width` px a row, holds 255
    /// where nothing was drawn, and each draw takes its coverage off
    /// (`v * (255 - a) / 255`, as `tint` does for black). A client's own px,
    /// which the frame holds only as a key colour, are darkened by it
    /// afterwards. It assumes a black shadow (`color` 0), as every shadow of
    /// the desk is: another colour does more than take light away. One pass,
    /// not a draw and a second walk over the same px: the second cost the
    /// full composite with Start and the pane open about a third (2026-11-28).
    pub fn draw_shaded(&self, c: &mut Canvas<'_>, shade: &mut [u8], r: Rect, clip: Rect) {
        debug_assert!(
            self.color << 8 == 0,
            "Shadow::draw_shaded needs a black shadow"
        );
        let stride = c.stride_px as usize;
        let width = c.width as usize;
        let bounds = c.bounds();
        self.each(r, clip.intersect(&bounds), |x, y, a| {
            let p = &mut c.pixels[y as usize * stride + x as usize];
            *p = tint(self.color, a, *p);
            if let Some(v) = shade.get_mut(y as usize * width + x as usize)
                && a != 0
            {
                *v = (u32::from(*v) * (255 - a) / 255) as u8;
            }
        });
    }

    /// Calls `f` with each px of the shadow in `clip` and its coverage.
    fn each(&self, r: Rect, clip: Rect, mut f: impl FnMut(i32, i32, u32)) {
        let (m, side) = (self.m, 3 * self.m);
        let outer = r.grow(m).intersect(&clip);
        let pick = |v: i32, lo: i32, hi: i32| {
            if v < lo + m {
                v - (lo - m)
            } else if v >= hi - m {
                v - (hi - m) + 2 * m
            } else {
                m + m / 2
            }
        };
        for y in outer.y..outer.bottom() {
            let sy = pick(y, r.y, r.bottom()).clamp(0, side - 1);
            for x in outer.x..outer.right() {
                if x >= r.x && x < r.right() && y >= r.y && y < r.bottom() {
                    continue;
                }
                let sx = pick(x, r.x, r.right()).clamp(0, side - 1);
                f(x, y, u32::from(self.alpha[(sy * side + sx) as usize]));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Image;

    #[test]
    fn shade_is_the_light_draw_shaded_leaves_of_white() {
        let shadow = Shadow::new(16, 120, 0);
        let mut img = Image::new(120, 120);
        let bounds = img.bounds();
        img.canvas().fill(bounds, 0xFFFF_FFFF);
        let mut shade = alloc::vec![255u8; 120 * 120];
        let window = Rect::new(40, 40, 40, 40);
        shadow.draw_shaded(&mut img.canvas(), &mut shade, window, bounds);
        assert!(shade.iter().any(|&v| v < 255));
        for (px, &v) in img.px.iter().zip(&shade) {
            assert_eq!(*px & 0xFF, u32::from(v));
        }
    }

    #[test]
    fn a_shadow_darkens_outside_the_window_and_fades() {
        let shadow = Shadow::new(16, 120, 0);
        let mut img = Image::new(200, 200);
        let bounds = img.bounds();
        img.canvas().fill(bounds, 0xFFFF_FFFF);
        let window = Rect::new(50, 50, 100, 100);
        shadow.draw(&mut img.canvas(), window, bounds);
        let at = |x: i32, y: i32| img.px[(y * 200 + x) as usize] & 0xFF;
        assert_eq!(img.px[100 * 200 + 100], 0xFFFF_FFFF, "inside is untouched");
        assert!(at(48, 100) < at(40, 100), "darker near the window");
        assert_eq!(at(20, 100), 255, "nothing past m");
    }
}
