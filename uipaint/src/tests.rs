//! The painter on a real window laid out with the real fonts: pixel goldens as
//! SHA-256 of the buffer (`UIPAINT_BLESS=1` rewrites them; check the PPM it
//! leaves in target/ before committing), and checks a golden cannot make.

use crate::{Measured, Painter, Scale};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use gfx2d::text::{Fonts, Text};
use gfx2d::{Image, Rect};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use uikit::list::Op;
use uikit::widget::{Axis, Insets, Layout, ROOT, Size, TextField, Widget};
use uikit::{Frame, Window};

const INTER: &[u8] = include_bytes!("../../assets/fonts/InterVariable.ttf");
const MONO: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Variable.ttf");

fn text(dark: bool) -> Text {
    Text::new(
        Fonts {
            sans: INTER,
            mono: MONO,
        },
        dark,
    )
    .unwrap()
}

fn frame(t: &Text) -> Frame {
    let mut w = Window::new("Notes", 320, 180);
    let column = Layout {
        padding: Insets::all(12),
        gap: 8,
        ..Layout::FILL
    };
    w.put(1, ROOT, column, Widget::Stack { axis: Axis::Column })
        .unwrap();
    let title = Widget::Label {
        text: "Groceries".into(),
        style: uikit::style::TypeStyle::Heading,
        tone: uikit::style::Tone::Normal,
    };
    w.put(2, 1, Layout::FIT, title).unwrap();
    let button = Widget::Button {
        text: "New note".into(),
        action: None,
    };
    w.put(3, 1, Layout::FIT, button).unwrap();
    let field = TextField {
        label: "Note".into(),
        value: "milk and eggs".into(),
        placeholder: String::new(),
        max_len: 200,
    };
    let fill = Layout {
        width: Size::Fill(1),
        ..Layout::FIT
    };
    w.put(4, 1, fill, Widget::TextLine(field)).unwrap();
    w.focus(4).unwrap();
    w.commit(&Measured(t))
}

fn render(scale: u32, palette: &look::Palette) -> (Image, Frame) {
    let mut t = text(palette.dark);
    let f = frame(&t);
    let s = Scale::of(scale);
    let size = s.rect(uikit::geom::Rect::new(0, 0, f.size.0, f.size.1));
    let (w, h) = (size.w, size.h);
    let mut img = Image::new(w, h);
    let mut p = Painter {
        palette,
        text: &mut t,
        scale: s,
    };
    let all = img.bounds();
    p.paint(&mut img.canvas(), &f.list, all);
    (img, f)
}

fn golden(name: &str, img: &Image) {
    let mut hash = Sha256::new();
    for p in &img.px {
        hash.update(p.to_le_bytes());
    }
    let got: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.sha256"));
    if std::env::var_os("UIPAINT_BLESS").is_some() {
        std::fs::write(&path, format!("{got}\n")).unwrap();
        let ppm = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target")
            .join(format!("uipaint-{name}.ppm"));
        let mut out: Vec<u8> = format!("P6\n{} {}\n255\n", img.w, img.h).into_bytes();
        for p in &img.px {
            out.extend([(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
        }
        std::fs::write(ppm, out).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(got, want.trim(), "{name}: set UIPAINT_BLESS=1 and review");
}

#[test]
fn a_window_matches_its_goldens_in_each_theme_and_scale() {
    for (name, palette, scale) in [
        ("dark-1x", &look::DARK, 120),
        ("light-1x", &look::LIGHT, 120),
        ("dark-1.5x", &look::DARK, 180),
        ("high-contrast-2x", &look::HIGH_CONTRAST, 240),
    ] {
        golden(name, &render(scale, palette).0);
    }
}

fn at(img: &Image, x: i32, y: i32) -> u32 {
    img.px[(y * img.w + x) as usize]
}

#[test]
fn fills_land_in_their_colour_and_text_draws_inside_its_box() {
    let palette = &look::DARK;
    let (img, f) = render(120, palette);
    let window = palette.window | 0xFF00_0000;
    assert_eq!(at(&img, 0, 0), window, "the background");
    let ops = f.list.groups.iter().flat_map(|g| &g.ops);
    for op in ops.clone() {
        if let Op::Fill { rect, token, .. } = op {
            let c = palette.get(crate::look_token(*token)) | 0xFF00_0000;
            let (x, y) = (rect.x + rect.w / 2, rect.y + rect.h / 2);
            // A fill's centre is its colour unless text is drawn over it.
            let covered = ops
                .clone()
                .any(|o| matches!(o, Op::Text { y: ty, .. } if (*ty..*ty + 24).contains(&y)));
            if !covered {
                assert_eq!(at(&img, x, y), c, "{op:?}");
            }
        }
    }
    let Some(Op::Text { x, y, .. }) = ops
        .clone()
        .find(|o| matches!(o, Op::Text { text, .. } if text == "Groceries"))
    else {
        panic!("no title");
    };
    let inked = (0..120)
        .flat_map(|dx| (0..24).map(move |dy| (x + dx, y + dy)))
        .any(|(px, py)| at(&img, px, py) != window);
    assert!(inked, "the title drew nothing");
}

#[test]
fn damage_limits_what_is_drawn() {
    let palette = &look::DARK;
    let mut t = text(true);
    let f = frame(&t);
    let mut img = Image::new(f.size.0, f.size.1);
    img.px.fill(0xFF12_3456);
    let mut p = Painter {
        palette,
        text: &mut t,
        scale: Scale::of(120),
    };
    let damage = Rect::new(0, 0, 10, 10);
    p.paint(&mut img.canvas(), &f.list, damage);
    assert_eq!(at(&img, 5, 5), palette.window | 0xFF00_0000);
    assert_eq!(at(&img, 50, 50), 0xFF12_3456);
}

#[test]
fn the_measure_is_the_fonts_and_scale_rounds_shared_edges() {
    let t = text(true);
    let m = Measured(&t);
    use uikit::layout::Measure;
    let (wide, _) = m.measure(uikit::style::TypeStyle::Body, "WWWW");
    let (narrow, line) = m.measure(uikit::style::TypeStyle::Body, "iiii");
    assert!(wide > narrow && line > 0.0, "{wide} {narrow}");
    let s = Scale::of(180);
    let a = s.rect(uikit::geom::Rect::new(0, 0, 3, 3));
    let b = s.rect(uikit::geom::Rect::new(3, 0, 3, 3));
    assert_eq!(a.right(), b.x);
}
