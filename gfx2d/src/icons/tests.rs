//! The Kai glyphs: each stays in its box and has ink at every desk size, a
//! stroke is never under 1 px, and drawing is deterministic. `DESK_PREVIEW=dir`
//! writes a contact sheet per theme there, as a PPM to look at.

use super::*;
use crate::canvas::Image;
use alloc::format;
use alloc::string::String;
use sha2::{Digest, Sha256};

fn digest(coverage: &[u8]) -> String {
    Sha256::digest(coverage)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn every_icon_stays_inside_its_box_at_each_desk_size() {
    const PAD: i32 = 4;
    for icon in Icon::ALL {
        for px in SIZES {
            let side = px + 2 * PAD;
            let cov = raster(icon, px, PAD);
            for (i, &a) in cov.iter().enumerate() {
                let (x, y) = (i as i32 % side, i as i32 / side);
                let inside = (PAD..PAD + px).contains(&x) && (PAD..PAD + px).contains(&y);
                assert!(inside || a == 0, "{icon:?} at {px} px inks ({x},{y})");
            }
        }
    }
}

#[test]
fn every_icon_keeps_the_safe_margin_at_24_px() {
    // At 24 px a unit is a pixel: nothing reaches the outer pixel ring, and
    // the 2-unit margin, less half a stroke, stays clear but for antialiasing.
    for icon in Icon::ALL {
        let m = render(icon, 24);
        for (i, &a) in m.coverage.iter().enumerate() {
            let (x, y) = (i as i32 % 24, i as i32 / 24);
            if x == 0 || y == 0 || x == 23 || y == 23 {
                assert_eq!(a, 0, "{icon:?} reaches the edge at ({x},{y})");
            }
        }
    }
}

#[test]
fn every_icon_has_ink_at_each_desk_size() {
    // At least half a stroke across the box in ink,
    // and a pixel at well over half coverage (a 1 px diagonal never fills one).
    for icon in Icon::ALL {
        for px in SIZES {
            let m = render(icon, px);
            let ink: u32 = m.coverage.iter().map(|&a| u32::from(a)).sum();
            assert!(ink >= 255 * px as u32 / 2, "{icon:?} at {px} px: ink {ink}");
            let peak = m.coverage.iter().max().copied().unwrap_or(0);
            assert!(peak >= 160, "{icon:?} at {px} px: peak {peak}");
        }
    }
}

#[test]
fn a_stroke_is_never_under_one_pixel() {
    assert_eq!(stroke_px(8), 1.0);
    assert_eq!(stroke_px(16), 1.0);
    assert_eq!(stroke_px(24), 1.5);
    assert_eq!(stroke_px(32), 2.0);
    for px in [1, 4, 10, 12, 15] {
        assert!(stroke_px(px) >= 1.0, "{px}");
    }
}

#[test]
fn a_second_draw_matches_the_first() {
    let mut seen = alloc::vec::Vec::new();
    for icon in Icon::ALL {
        for px in SIZES {
            let (a, b) = (render(icon, px), render(icon, px));
            assert_eq!(digest(&a.coverage), digest(&b.coverage), "{icon:?} {px}");
            seen.push(digest(&a.coverage));
        }
    }
    // No two glyphs, or sizes of one, come out the same.
    let all = seen.len();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), all);
}

#[test]
fn names_are_unique_and_round_trip() {
    for icon in Icon::ALL {
        assert_eq!(Icon::from_name(icon.name()), Some(icon));
    }
    assert_eq!(Icon::from_name("chevron-down"), Some(Icon::ChevronDown));
    assert_eq!(Icon::from_name("lucide-x"), None);
    assert_eq!(Icon::from_name(""), None);
}

#[test]
fn the_cache_makes_each_mask_once() {
    let mut icons = Icons::default();
    assert!(icons.is_empty());
    let first = icons.get(Icon::Kai, 20).clone();
    icons.get(Icon::Kai, 20);
    icons.get(Icon::Kai, 24);
    assert_eq!(icons.len(), 2);
    assert_eq!(*icons.get(Icon::Kai, 20), first);
    assert_eq!(first, render(Icon::Kai, 20));
}

#[test]
fn a_mask_tints_only_its_ink_and_stays_in_the_clip() {
    let mut img = Image::new(40, 40);
    let mut c = img.canvas();
    c.fill(c.bounds(), 0xFF00_0000);
    let mut icons = Icons::default();
    // Half off the canvas to the upper left, and clipped on the right.
    let clip = Rect::new(0, 0, 10, 40);
    icons.draw(&mut c, (Icon::Close, 24), (-8, -8), 0x00FF_FFFF, clip);
    for y in 0..40 {
        for x in 0..40 {
            let p = img.px[(y * 40 + x) as usize];
            assert_eq!(p >> 24, 0xFF, "alpha kept at ({x},{y})");
            if x >= 10 || y >= 16 {
                assert_eq!(p, 0xFF00_0000, "outside the clip or mask at ({x},{y})");
            }
        }
    }
    assert!(img.px.contains(&0xFFFF_FFFF), "the cross inks");
}

/// The contact sheet: every glyph at each desk size, one row per glyph, in
/// `text` on `window`.
fn sheet(palette: &look::Palette) -> Image {
    const CELL: i32 = 40;
    let w = CELL * SIZES.len() as i32;
    let h = CELL * Icon::ALL.len() as i32;
    let mut img = Image::new(w, h);
    let mut c = img.canvas();
    c.fill(c.bounds(), palette.get(look::Token::Window) | 0xFF00_0000);
    let mut icons = Icons::default();
    let ink = palette.get(look::Token::Text);
    let kai = palette.get(look::Token::Ai);
    for (row, icon) in Icon::ALL.into_iter().enumerate() {
        for (col, px) in SIZES.into_iter().enumerate() {
            let (x, y) = (col as i32 * CELL, row as i32 * CELL);
            let at = (x + (CELL - px) / 2, y + (CELL - px) / 2);
            let color = if icon == Icon::Kai { kai } else { ink };
            let clip = c.bounds();
            icons.draw(&mut c, (icon, px), at, color, clip);
        }
    }
    img
}

#[test]
fn the_contact_sheet_draws_in_each_theme() {
    for (name, palette) in [
        ("dark", &look::DARK),
        ("light", &look::LIGHT),
        ("high-contrast", &look::HIGH_CONTRAST),
    ] {
        let img = sheet(palette);
        let ground = palette.get(look::Token::Window) | 0xFF00_0000;
        assert!(img.px.iter().any(|&p| p != ground), "{name}");
        let Ok(dir) = std::env::var("DESK_PREVIEW") else {
            continue;
        };
        let mut out: alloc::vec::Vec<u8> = format!("P6\n{} {}\n255\n", img.w, img.h).into_bytes();
        for p in &img.px {
            out.extend([(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
        }
        let path = std::path::Path::new(&dir).join(format!("icons-{name}.ppm"));
        std::fs::write(path, out).unwrap();
    }
}
