//! The `display` boot cases' test pattern, which the kernel draws and xtask
//! compares with a QEMU screendump pixel for pixel, and the hardware cursor's
//! image. Pixels are `0x00RRGGBB`.
//!
//! The pattern is eight colour bars over the top half, an 8-pixel checkerboard
//! over the next quarter and a grey ramp left to right over the bottom quarter,
//! inside a one-pixel orange frame. The frame catches an image shifted or cut
//! off by a pixel; the checkerboard, a wrong stride; the ramp, swapped channels.

/// The colour bars, left to right.
pub const BARS: [u32; 8] = [
    0x00FF_FFFF,
    0x00FF_FF00,
    0x0000_FFFF,
    0x0000_FF00,
    0x00FF_00FF,
    0x00FF_0000,
    0x0000_00FF,
    0x0000_0000,
];
pub const FRAME: u32 = 0x00FF_8000;
const CHECK_DARK: u32 = 0x0020_2020;
const CHECK_LIGHT: u32 = 0x00C0_C0C0;
const CHECK: u32 = 8;

/// The pattern's pixel at `x`, `y` on a `width` by `height` screen.
pub fn pixel(x: u32, y: u32, width: u32, height: u32) -> u32 {
    if x == 0 || y == 0 || x + 1 == width || y + 1 == height {
        return FRAME;
    }
    if y < height / 2 {
        return BARS[(x * 8 / width) as usize];
    }
    if y < height * 3 / 4 {
        return match (x / CHECK + y / CHECK) % 2 {
            0 => CHECK_DARK,
            _ => CHECK_LIGHT,
        };
    }
    let grey = x * 255 / (width - 1);
    grey << 16 | grey << 8 | grey
}

/// Where the boot case first shows the cursor's hot spot, and where it moves it.
pub const CURSOR_AT: (u32, u32) = (100, 100);
pub const CURSOR_TO: (u32, u32) = (300, 200);

/// The cursor image's side in pixels, which virtio-gpu fixes.
pub const CURSOR: u32 = 64;

/// An arrow pointing up and left, hot spot at its tip (0, 0): white with a black
/// edge, transparent elsewhere. Pixels are `0xAARRGGBB`.
pub fn cursor_pixel(x: u32, y: u32) -> u32 {
    const OPAQUE: u32 = 0xFF00_0000;
    // A right triangle 20 rows tall with a 12-pixel-wide base, plus a stem.
    let in_head = y < 20 && x <= y * 12 / 20;
    let in_stem = (12..28).contains(&y) && (4..8).contains(&x);
    if !in_head && !in_stem {
        return 0;
    }
    let edge = x == 0 || (in_head && (y == 19 || x == y * 12 / 20)) || (in_stem && !in_head);
    match edge {
        true => OPAQUE,
        false => OPAQUE | 0x00FF_FFFF,
    }
}

#[cfg(test)]
mod tests {
    use super::{BARS, CURSOR, FRAME, cursor_pixel, pixel};

    #[test]
    fn frame_bars_checkerboard_and_ramp_land_where_documented() {
        let (w, h) = (1024, 768);
        assert_eq!(pixel(0, 300, w, h), FRAME);
        assert_eq!(pixel(1023, 300, w, h), FRAME);
        assert_eq!(pixel(500, 767, w, h), FRAME);
        assert_eq!(pixel(1, 1, w, h), BARS[0]);
        assert_eq!(pixel(1022, 383, w, h), BARS[7]);
        assert_eq!(
            pixel(8 * 3, 384, w, h),
            pixel(8 * 3, 384 + 8, w, h) ^ 0x00E0_E0E0
        );
        assert_eq!(pixel(1, 700, w, h), 0x0000_0000);
        assert_eq!(pixel(1022, 700, w, h), 0x00FE_FEFE);
    }

    #[test]
    fn cursor_is_opaque_at_its_tip_and_clear_far_from_it() {
        assert_eq!(cursor_pixel(0, 0) >> 24, 0xFF);
        assert_eq!(cursor_pixel(3, 10), 0xFFFF_FFFF);
        assert_eq!(cursor_pixel(CURSOR - 1, CURSOR - 1), 0);
        assert_eq!(cursor_pixel(30, 5), 0);
    }
}
