//! Drawing on a 32-bpp linear framebuffer: rectangles, glyphs, and the small shapes
//! the status bar uses. Every write goes straight to the framebuffer, each pixel
//! once, so nothing flickers between a fill and the text over it.
//!
//! The kernel builds unoptimised, where each iterator step is a call, so the loops
//! that touch every pixel walk raw pointers with `while`: under QEMU's TCG a log
//! line otherwise costs a printing process tens of milliseconds of its CPU burst.

use super::layout::Font;
use core::ptr::NonNull;

#[derive(Clone, Copy)]
pub struct Fb {
    pub base: NonNull<u32>,
    /// Pixels per framebuffer row (pitch / 4).
    pub stride: usize,
}

impl Fb {
    fn put(self, x: usize, y: usize, color: u32) {
        // SAFETY: callers keep x and y inside the framebuffer Limine mapped.
        unsafe { self.at(x, y).write_volatile(color) };
    }

    /// The address of pixel (x, y).
    fn at(self, x: usize, y: usize) -> *mut u32 {
        self.base.as_ptr().wrapping_add(y * self.stride + x)
    }

    pub fn fill(self, x0: usize, y0: usize, width: usize, height: usize, color: u32) {
        let mut line = self.at(x0, y0);
        for _ in 0..height {
            let (mut pixel, end) = (line, line.wrapping_add(width));
            while pixel < end {
                // SAFETY: callers keep the rectangle inside the framebuffer.
                unsafe { pixel.write_volatile(color) };
                pixel = pixel.wrapping_add(1);
            }
            line = line.wrapping_add(self.stride);
        }
    }

    /// Draws `ch` with its top left at `at`, blended from `ink` (a colour, and
    /// whether bold) into `bg` by its coverage; a blank fills the cell with `bg`.
    pub fn glyph(self, font: Font, at: (usize, usize), ch: char, ink: (u32, bool), bg: u32) {
        let ((x0, y0), (fg, bold)) = (at, ink);
        let Some(glyph) = font.glyph(ch, bold) else {
            return self.fill(x0, y0, font.width, font.height, bg);
        };
        let mut out = self.at(x0, y0);
        for line in glyph.raster() {
            let (mut pixel, mut i) = (out, 0);
            while i < line.len() {
                // SAFETY: callers keep the cell inside the framebuffer.
                unsafe { pixel.write_volatile(blend(line[i], fg, bg)) };
                (pixel, i) = (pixel.wrapping_add(1), i + 1);
            }
            out = out.wrapping_add(self.stride);
        }
    }

    /// Writes `text` from (x, y) and returns the x after it.
    pub fn text(
        self,
        font: Font,
        at: (usize, usize),
        text: &str,
        ink: (u32, bool),
        bg: u32,
    ) -> usize {
        let (x, y) = at;
        text.chars().fold(x, |x, ch| {
            self.glyph(font, (x, y), ch, ink, bg);
            x + font.width
        })
    }

    /// A filled disc of `radius` centred on (cx, cy), its edge pixels blended.
    pub fn disc(self, cx: usize, cy: usize, radius: usize, color: u32, bg: u32) {
        let r = radius as isize;
        // Squared distances in quarter pixels: inside, on the edge, outside.
        let (inner, outer) = ((4 * r - 2).pow(2), (4 * r + 2).pow(2));
        for dy in -r..=r {
            for dx in -r..=r {
                let d = (4 * dx).pow(2) + (4 * dy).pow(2);
                let coverage = match d {
                    d if d <= inner => 255,
                    d if d >= outer => 0,
                    d => 255 * (outer - d) / (outer - inner),
                };
                let (x, y) = (cx as isize + dx, cy as isize + dy);
                self.put(x as usize, y as usize, blend(coverage as u8, color, bg));
            }
        }
    }

    /// Moves `count` pixel rows starting at `from` up to `to`.
    pub fn move_up(self, from: usize, to: usize, count: usize) {
        // SAFETY: both ranges lie inside the framebuffer; ptr::copy handles overlap.
        unsafe {
            core::ptr::copy(
                self.base.as_ptr().add(from * self.stride),
                self.base.as_ptr().add(to * self.stride),
                count * self.stride,
            );
        }
    }
}

/// Mixes foreground into background by glyph coverage, per colour channel.
pub fn blend(intensity: u8, fg: u32, bg: u32) -> u32 {
    // Most of a glyph's pixels are empty or full.
    match intensity {
        0 => return bg,
        255 => return fg,
        _ => {}
    }
    let mix = |shift: u32| {
        let fg = (fg >> shift) & 0xFF;
        let bg = (bg >> shift) & 0xFF;
        let value = (bg * (255 - u32::from(intensity)) + fg * u32::from(intensity)) / 255;
        value << shift
    };
    mix(16) | mix(8) | mix(0)
}
