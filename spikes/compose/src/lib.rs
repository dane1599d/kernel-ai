//! Spike S05-1 "compose and look" (ADR 0005): a throwaway prototype of `gfx2d`
//! and `desktop`'s compositing, no_std on tiny-skia 0.12.0, skrifa 0.48.0 and
//! harfrust 0.14.0, plus vello_cpu 0.3.0 for scenes (a) and (b). The ring-3
//! program `compose` (user/src/bin/compose.rs) runs it on the real display and
//! times it; the host tests check that each scene draws. Never merged.
//!
//! - [`look`]: ADR 0005 §8's colour tokens, dark and light;
//! - [`frame`]: an XRGB8888 frame, rectangles, fills, premultiplied blending and
//!   the nine-slice shadow;
//! - [`text`]: shaping (harfrust), outlines (skrifa) filled by tiny-skia into a
//!   glyph atlas keyed by (font, glyph, size in 1/64 px, x phase in quarters),
//!   with light vertical hinting at scale 1 and the coverage table;
//! - [`desk`]: windows with server-side chrome, the wallpaper, the top bar, the
//!   panel in each state, and damage-tracked compositing;
//! - [`scenes`]: scenes (a)-(d) and (f), each 300 frames (the sheet's 17-frame
//!   spring repeated), and (e)'s client drawing;
//! - [`vello`]: scenes (a) and (b) drawn by vello_cpu;
//! - [`stats`], percentiles.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod desk;
pub mod frame;
pub mod look;
pub mod scenes;
pub mod stats;
pub mod text;
pub mod vello;

/// The fonts, embedded: ADR 0005's two families (OFL-1.1), Inter 4.1 and
/// JetBrains Mono 2.304, both variable. `KAI_FONTS` names the folder holding
/// `InterVariable.ttf` and `JetBrainsMono.ttf`, fetched by SHA-256.
pub const INTER: &[u8] = include_bytes!(concat!(env!("KAI_FONTS"), "/InterVariable.ttf"));
pub const MONO: &[u8] = include_bytes!(concat!(env!("KAI_FONTS"), "/JetBrainsMono.ttf"));
