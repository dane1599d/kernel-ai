//! The scale (ADR 0005 §9), in 120ths as fractional-scale-v1 carries it: from
//! the EDID's physical size when there is one, else from the width (up to 1920
//! wide 1.0, up to 2560 1.25, up to 3840 2.0), in quarter steps from 1.0 to 3.0.

/// 1.0.
pub const ONE: u32 = 120;
const STEP: u32 = 30;
const MAX: u32 = 360;
/// The density 1.0 stands for: 96 logical px per inch, in px per 10 m.
const BASE_PX_PER_10M: u64 = 96 * 10_000 * 10 / 254;

/// The scale for a screen `width` px wide and `width_mm` across (0 unknown).
pub fn for_screen(width: u32, width_mm: u32) -> u32 {
    let raw = match width_mm {
        0 => match width {
            0..=1920 => 120,
            1921..=2560 => 150,
            _ => 240,
        },
        mm => {
            // Pixels per 10 m, then the ratio to the base density.
            let density = u64::from(width) * 10_000 / u64::from(mm);
            (density * u64::from(ONE) / BASE_PX_PER_10M) as u32
        }
    };
    let stepped = (raw + STEP / 2) / STEP * STEP;
    stepped.clamp(ONE, MAX)
}

/// Physical px for `logical` at `scale`.
pub fn to_physical(logical: i32, scale: u32) -> i32 {
    (i64::from(logical) * i64::from(scale) / i64::from(ONE)) as i32
}

/// Logical px for `physical` at `scale`.
pub fn to_logical(physical: i32, scale: u32) -> i32 {
    (i64::from(physical) * i64::from(ONE) / i64::from(scale.max(1))) as i32
}
