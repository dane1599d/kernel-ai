//! Comparing two screenshots as wptrunner does (`executors/base.py`,
//! `get_differences` and `check_pass`).
//!
//! The images are compared as RGB, alpha dropped. Two figures come out:
//! - the largest difference in any one channel;
//! - the count of pixels that differ at all.
//!
//! With no fuzzy allowance, or an allowance of `0-0;0-0`, two images are equal
//! only if no pixel differs. With one, they are equal if either:
//! - no pixel differs and the pixel range starts at 0;
//! - nothing differs in any channel and the channel range starts at 0; or
//! - both figures fall inside their ranges.
//!
//! Images of different sizes are never equal.

use super::manifest::{Fuzzy, Relation};

/// A screenshot: RGBA8, row by row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Picture {
    /// A picture of one colour.
    pub fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        Picture {
            width,
            height,
            rgba: rgba.repeat(width as usize * height as usize),
        }
    }
}

/// How two pictures differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Difference {
    /// The largest difference in one of R, G or B, over every pixel.
    pub per_channel: u32,
    /// Pixels where R, G or B differ.
    pub pixels: u64,
    /// The sizes differ: nothing else was compared.
    pub size: bool,
}

/// How `a` and `b` differ, as RGB.
pub fn differences(a: &Picture, b: &Picture) -> Difference {
    if (a.width, a.height) != (b.width, b.height) || a.rgba.len() != b.rgba.len() {
        return Difference {
            per_channel: 255,
            pixels: u64::from(a.width.max(b.width)) * u64::from(a.height.max(b.height)),
            size: true,
        };
    }
    let mut per_channel = 0;
    let mut pixels = 0;
    for (p, q) in a
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0)
    {
        let d = (0..3)
            .map(|i| u32::from(p[i].abs_diff(q[i])))
            .max()
            .unwrap_or(0);
        if d > 0 {
            pixels += 1;
            per_channel = per_channel.max(d);
        }
    }
    Difference {
        per_channel,
        pixels,
        size: false,
    }
}

/// Whether `diff` counts as equal under `fuzzy` (wptrunner's `check_pass`).
pub fn equal(diff: Difference, fuzzy: Option<Fuzzy>) -> bool {
    if diff.size {
        return false;
    }
    let exact = Fuzzy {
        per_channel: (0, 0),
        pixels: (0, 0),
    };
    match fuzzy.filter(|f| *f != exact) {
        None => diff.pixels == 0 && diff.per_channel == 0,
        Some(f) => {
            (diff.pixels == 0 && f.pixels.0 == 0)
                || (diff.per_channel == 0 && f.per_channel.0 == 0)
                || ((f.per_channel.0..=f.per_channel.1).contains(&diff.per_channel)
                    && (f.pixels.0..=f.pixels.1).contains(&diff.pixels))
        }
    }
}

/// Whether one comparison passes: equal for `==`, not equal for `!=`.
pub fn passes(diff: Difference, fuzzy: Option<Fuzzy>, relation: Relation) -> bool {
    equal(diff, fuzzy) == (relation == Relation::Match)
}
