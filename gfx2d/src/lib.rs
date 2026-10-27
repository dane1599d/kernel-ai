//! Kernel AI's 2D renderer (ADR 0005 §5): what `desktop` and uikit draw with.
//!
//! - [`canvas`]: a [`Canvas`] over 32-bit pixels (`0xAARRGGBB`, premultiplied
//!   alpha, sRGB, row 0 at the top: `surfabi`'s formats), the owned [`Image`],
//!   and the blits compositing needs: fills, opaque copies, premultiplied
//!   source-over, and window layers whose opaque rows copy and whose rounded
//!   bands blend;
//! - [`boxdraw`]: box drawing and block elements, U+2500-259F, drawn by code
//!   per cell so lines meet at cell edges at every size;
//! - [`damage`]: [`Damage`], at most `N` rectangles, past that their bounding box
//!   (clients 32, `desktop` 16);
//! - [`shadow`]: pre-rendered nine-slice shadows, so a moving shadow is a blit;
//! - [`text`]: harfrust shapes, skrifa draws outlines and tiny-skia fills them to
//!   8-bit coverage, kept in an atlas keyed by (font, glyph, size in 1/64 px, x
//!   phase in quarters, weight); light vertical hinting at scale 1, none above;
//!   greyscale only, through the theme's coverage table. Printable-ASCII
//!   monospace text takes the cell path: no shaping, no ligatures.
//!
//! Sizes from `look` are logical px; the scale (`n / 120`, fractional-scale-v1)
//! turns them into the buffer's physical px, rounded only at glyph and rectangle
//! edges. S05-1 (#176) measured this stack under KVM: a full 1920x1080 composite
//! in 1.82 ms and a warm 160x45 text page in 3.19 ms, both at p99.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod boxdraw;
pub mod canvas;
pub mod damage;
pub mod shadow;
pub mod text;

pub use canvas::{Canvas, Image, Rect};
pub use damage::Damage;
pub use text::{Fonts, Text};

/// The scale as fractional-scale-v1 sends it: `n / 120`.
pub fn scale(n120: u32) -> f32 {
    n120 as f32 / 120.0
}

/// `logical` px at `scale`, rounded.
pub fn px(logical: f32, scale: f32) -> i32 {
    libm::roundf(logical * scale) as i32
}

#[cfg(test)]
pub(crate) mod testfonts {
    /// The fonts ADR 0005 §8 names, as committed in assets/fonts/ (OFL-1.1).
    pub const INTER: &[u8] = include_bytes!("../../assets/fonts/InterVariable.ttf");
    pub const MONO: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Variable.ttf");
}
