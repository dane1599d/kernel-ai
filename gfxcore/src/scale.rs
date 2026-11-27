//! Whether a text cell is a sensible size on the screen it is on. The console
//! takes the largest font that leaves its columns (a pixel count), which says
//! nothing of how big the glyphs are to look at: on a 55-inch panel at 1080p a
//! 24-pixel row is 15 mm tall. When the screen's EDID gives its size, a font
//! taller than [`MAX_ROW_MM`] is passed over for a smaller one that still fits.

use crate::edid::Mode;

/// The tallest text row, in mm, the console picks when it knows the screen's
/// size. QEMU's EDID density is about 100 dpi, where the largest font (32 px) is
/// 8.1 mm, so nothing a virtual screen shows is passed over.
pub const MAX_ROW_MM: u32 = 9;

/// Whether a font `height` pixels tall on a screen `width` pixels across suits a
/// screen of `size_mm` (width by height) millimetres. True when the size is
/// unknown or the width is 0.
pub fn suits(height: usize, width: usize, size_mm: Option<Mode>) -> bool {
    let Some((width_mm, _)) = size_mm else {
        return true;
    };
    if width == 0 {
        return true;
    }
    // height * width_mm / width <= MAX_ROW_MM, without a division.
    (height as u64) * u64::from(width_mm) <= u64::from(MAX_ROW_MM) * (width as u64)
}

#[cfg(test)]
mod tests {
    use super::suits;

    #[test]
    fn a_screen_of_unknown_size_takes_any_font() {
        assert!(suits(32, 1920, None));
        assert!(suits(32, 0, Some((500, 300))));
    }

    #[test]
    fn a_laptop_panel_keeps_the_font_the_width_gave() {
        // 13.3": 1920 px over 294 mm (~166 dpi): 24 px is 3.7 mm.
        assert!(suits(24, 1920, Some((294, 165))));
        // A 24" 1080p desk monitor, 531 mm: 24 px is 6.6 mm.
        assert!(suits(24, 1920, Some((531, 299))));
        assert!(suits(32, 2560, Some((597, 336))));
    }

    #[test]
    fn qemus_density_passes_over_none_of_the_fonts() {
        // 100 dpi is 3.94 px/mm: each mode's width in mm at that density.
        for (px, width) in [(1280, 1280), (1600, 1600), (1920, 1920), (2560, 2560)] {
            let mm = (width as f64 / 3.937) as u32;
            let font = if px >= 2560 { 32 } else { 24 };
            assert!(suits(font, px, Some((mm, mm * 9 / 16))), "{px}");
        }
    }

    #[test]
    fn a_big_tv_at_1080p_passes_over_the_large_fonts() {
        // 55": 1217 mm across, 1920 px: 24 px is 15 mm, 20 px 12.7, 16 px 10.1.
        assert!(!suits(24, 1920, Some((1217, 684))));
        assert!(!suits(20, 1920, Some((1217, 684))));
        // A 40" 1080p TV: 885 mm; 16 px is 7.4 mm.
        assert!(suits(16, 1920, Some((885, 498))));
        assert!(!suits(24, 1920, Some((885, 498))));
    }
}
