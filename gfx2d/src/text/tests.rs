use super::*;
use crate::canvas::Image;
use crate::testfonts::{INTER, MONO};
use look::{DARK, LIGHT, Palette};
use std::format;
use std::string::String;

fn text(dark: bool) -> Text {
    Text::new(
        Fonts {
            sans: INTER,
            mono: MONO,
        },
        dark,
    )
    .expect("the committed fonts parse")
}

#[test]
fn the_cell_path_lays_monospace_out_cell_by_cell() {
    let t = text(true);
    let (run, _) = t.run(TypeStyle::Mono, 1.0);
    let (cells, advance) = t.shape(run, "kai> ls");
    assert_eq!(cells.len(), 7);
    let cell = cells[1].1 - cells[0].1;
    assert!(
        cell > 7.0 && cell < 10.0,
        "a 14 px JetBrains Mono cell: {cell}"
    );
    for (i, (_, at)) in cells.iter().enumerate() {
        assert!((at - cell * i as f32).abs() < 1e-3);
    }
    assert!((advance - 7.0 * cell).abs() < 1e-3);
    // The shaper agrees where no ligature applies.
    let (shaped, shaped_advance) = t.shape(run, "kai\u{e9}");
    assert_eq!(shaped.len(), 4);
    assert!((shaped_advance - 4.0 * cell).abs() < 1e-2);
}

#[test]
fn measure_gives_logical_advance_and_the_style_line() {
    let t = text(true);
    let (w, line) = t.measure(TypeStyle::Body, "Groceries");
    assert!(w > 40.0 && w < 90.0, "{w}");
    assert_eq!(line, 20.0);
    assert_eq!(t.measure(TypeStyle::Heading, "x").1, 24.0);
}

#[test]
fn drawing_reuses_the_atlas() {
    let mut t = text(true);
    let mut img = Image::new(400, 40);
    let all = img.bounds();
    t.draw(
        &mut img.canvas(),
        all,
        4.0,
        24,
        TypeStyle::Mono,
        1.0,
        DARK.text,
        "make a note",
    );
    let first = t.misses;
    assert!(first > 0);
    // The same run again, and a cell later (same phases for each glyph).
    t.draw(
        &mut img.canvas(),
        all,
        4.0,
        24,
        TypeStyle::Mono,
        1.0,
        DARK.text,
        "make a note",
    );
    assert_eq!(t.misses, first, "every glyph was in the atlas");
    assert!(
        img.px.iter().any(|p| (p >> 16) & 255 > 0xC0),
        "the text is drawn, some pixels near full coverage"
    );
}

#[test]
fn text_stays_inside_the_clip() {
    let mut t = text(true);
    let mut img = Image::new(200, 40);
    let clip = Rect::new(20, 0, 30, 40);
    t.draw(
        &mut img.canvas(),
        clip,
        0.0,
        28,
        TypeStyle::Heading,
        1.0,
        DARK.text,
        "WWWWWWWWWW",
    );
    for y in 0..40 {
        for x in 0..200 {
            if !(20..50).contains(&x) {
                assert_eq!(img.px[y * 200 + x], 0, "({x}, {y}) is outside the clip");
            }
        }
    }
}

/// FNV-1a 64 of a picture's pixels.
fn digest(img: &Image) -> u64 {
    img.px.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, p| {
        p.to_le_bytes().iter().fold(h, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
        })
    })
}

/// A sample for each theme: every type style, at scale 1 and 1.25.
fn sample(palette: Palette) -> Image {
    let mut t = text(palette.dark);
    let mut img = Image::new(640, 300);
    let all = img.bounds();
    img.canvas().fill(all, palette.window | 0xFF00_0000);
    let mut y = 24;
    for (i, style) in TypeStyle::ALL.iter().enumerate() {
        let scale = if i % 2 == 0 { 1.0 } else { 1.25 };
        let line = format!("{style:?}: kai> make a note called groceries 0123");
        t.draw(
            &mut img.canvas(),
            all,
            12.0,
            y,
            *style,
            scale,
            palette.text,
            &line,
        );
        y += 38;
    }
    img
}

/// Writes `img` as a PPM under target/ for a look, and says where.
fn keep(img: &Image, name: &str) -> String {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target");
    let path = dir.join(format!("gfx2d-{name}.ppm"));
    let mut bytes = format!("P6\n{} {}\n255\n", img.w, img.h).into_bytes();
    for p in &img.px {
        bytes.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&path, bytes);
    format!("{}", path.display())
}

/// The golden samples, as digests: a change to glyph rendering, hinting, the
/// coverage tables or blending changes them. A mismatch writes the picture
/// under target/ to look at before the digest is updated.
#[test]
fn the_samples_match_their_goldens() {
    for (palette, want) in [
        (DARK, 0x67a0_3699_2f25_251cu64),
        (LIGHT, 0xaf64_52c7_27cf_e5bd),
    ] {
        let img = sample(palette);
        let got = digest(&img);
        if got != want {
            let at = keep(&img, palette.name);
            panic!(
                "{} sample digest {got:#018x}, golden {want:#018x}; see {at}",
                palette.name
            );
        }
    }
}

#[test]
fn the_committed_fonts_are_the_pinned_ones() {
    use sha2::{Digest, Sha256};
    let sums = include_str!("../../../assets/fonts/SHA256SUMS");
    for (bytes, name) in [
        (INTER, "InterVariable.ttf"),
        (MONO, "JetBrainsMono-Variable.ttf"),
    ] {
        let hex: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(sums.contains(&format!("{hex}  {name}")), "{name}: {hex}");
    }
}
