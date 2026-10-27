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
        let (m, side) = (self.m, 3 * self.m);
        let outer = r.grow(m).intersect(&clip).intersect(&c.bounds());
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
            let row = y as usize * c.stride_px as usize;
            for x in outer.x..outer.right() {
                if x >= r.x && x < r.right() && y >= r.y && y < r.bottom() {
                    continue;
                }
                let sx = pick(x, r.x, r.right()).clamp(0, side - 1);
                let a = u32::from(self.alpha[(sy * side + sx) as usize]);
                let p = &mut c.pixels[row + x as usize];
                *p = tint(self.color, a, *p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Image;

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
