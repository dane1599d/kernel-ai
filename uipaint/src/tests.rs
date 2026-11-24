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
use uikit::layout::Measure;
use uikit::list::Op;
use uikit::widget::{Axis, Insets, Layout, ROOT, Size, TextField, Widget};
use uikit::{Frame, Window};

const INTER: &[u8] = include_bytes!("../../assets/fonts/InterVariable.ttf");
const MONO: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Variable.ttf");

/// A colour of no theme, written by its channels (the lint takes no literals).
const TEAL: u32 = crate::OPAQUE | (0x12 << 16) | (0x34 << 8) | 0x56;

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

/// The theme the desktop paints in for saved `settings` lines (`kai-appearance
/// 1`'s `setting = value`), resolved as comphost does from `Told::Look`.
fn appearance(settings: &str) -> look::theme::Theme {
    use look::appearance::{Appearance, Shipped};
    let text = format!(
        "kai-appearance 1
{settings}
"
    );
    Appearance::decode(&text, &Shipped)
        .and_then(|a| a.resolve(&Shipped))
        .unwrap()
}

/// [`render`] in `theme`: its colours, and the window laid out and drawn in its
/// type (text size and legible), as comphost does once the desktop says it.
fn render_in(theme: &look::theme::Theme, scale: u32) -> (Image, Frame) {
    let palette = theme.palette();
    let mut t = text(palette.dark);
    t.set_theme(theme);
    let f = frame(&t);
    let s = Scale::of(scale);
    let size = s.rect(uikit::geom::Rect::new(0, 0, f.size.0, f.size.1));
    let mut img = Image::new(size.w, size.h);
    let mut p = Painter {
        palette: &palette,
        text: &mut t,
        scale: s,
    };
    let all = img.bounds();
    p.paint(&mut img.canvas(), &f.list, all);
    (img, f)
}

/// The top of the last line of text in a frame: where the layout has got to.
fn last_text_y(f: &Frame) -> i32 {
    f.list
        .groups
        .iter()
        .flat_map(|g| &g.ops)
        .filter_map(|op| match op {
            Op::Text { y, .. } => Some(*y),
            _ => None,
        })
        .max()
        .unwrap()
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
    let window = palette.window | crate::OPAQUE;
    assert_eq!(at(&img, 0, 0), window, "the background");
    let ops = f.list.groups.iter().flat_map(|g| &g.ops);
    for op in ops.clone() {
        if let Op::Fill { rect, token, .. } = op {
            let c = palette.get(crate::look_token(*token)) | crate::OPAQUE;
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
    img.px.fill(TEAL);
    let mut p = Painter {
        palette,
        text: &mut t,
        scale: Scale::of(120),
    };
    let damage = Rect::new(0, 0, 10, 10);
    p.paint(&mut img.canvas(), &f.list, damage);
    assert_eq!(at(&img, 5, 5), palette.window | crate::OPAQUE);
    assert_eq!(at(&img, 50, 50), TEAL);
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

#[test]
fn an_icon_draws_its_kai_glyph_and_an_unknown_name_its_frame() {
    use uikit::geom::Rect as Logical;
    use uikit::list::{DisplayList, Group};
    use uikit::style::Token;
    let palette = &look::DARK;
    let mut t = text(true);
    let icon = |x: i32, name: &'static str| Op::Icon {
        rect: Logical::new(x, 10, 16, 16),
        name,
        token: Token::Text,
    };
    let all = Logical::new(0, 0, 80, 40);
    let list = DisplayList {
        groups: alloc::vec![Group {
            node: 1,
            clip: all,
            bounds: all,
            ops: alloc::vec![icon(10, "chevron-down"), icon(40, "lucide-x")],
        }],
    };
    let mut img = Image::new(80, 40);
    let mut p = Painter {
        palette,
        text: &mut t,
        scale: Scale::of(120),
    };
    let bounds = img.bounds();
    p.paint(&mut img.canvas(), &list, bounds);
    let (window, ink) = (palette.window | crate::OPAQUE, palette.text | crate::OPAQUE);
    // The chevron: ink where its mask has any to show, none where it has none.
    let mask = gfx2d::icons::render(gfx2d::icons::Icon::ChevronDown, 16);
    for (i, &a) in mask.coverage.iter().enumerate() {
        let (x, y) = (10 + i as i32 % 16, 10 + i as i32 / 16);
        match a {
            0 => assert_eq!(at(&img, x, y), window, "({x},{y})"),
            32.. => assert_ne!(at(&img, x, y), window, "({x},{y}) coverage {a}"),
            _ => {}
        }
    }
    assert_eq!(at(&img, 10, 10), window);
    // No glyph by that name: the 1 px frame.
    assert_eq!(at(&img, 40, 10), ink);
    assert_eq!(at(&img, 55, 25), ink);
    assert_eq!(at(&img, 47, 17), window);
}

/// Text size and legible type reach the layout, not only the paint: at 150% the
/// window's rows are taller and each line is lower, and the type the layout
/// measured is the one drawn.
#[test]
fn a_window_laid_out_at_150_percent_has_taller_rows() {
    let plain = appearance("theme = dark");
    let big = appearance(
        "theme = dark
text_size = 150
legible = on",
    );
    let (_, f100) = render_in(&plain, 120);
    let (_, f150) = render_in(&big, 120);
    assert!(
        last_text_y(&f150) > last_text_y(&f100) + 12,
        "{} against {}",
        last_text_y(&f150),
        last_text_y(&f100)
    );
    // The measure agrees with the paint: a line is wider at 150%.
    let mut a = text(true);
    a.set_theme(&plain);
    let mut b = text(true);
    b.set_theme(&big);
    let style = uikit::style::TypeStyle::Heading;
    let (wa, ha) = Measured(&a).measure(style, "Groceries");
    let (wb, hb) = Measured(&b).measure(style, "Groceries");
    assert!(wb > wa * 1.4 && hb > ha * 1.4, "{wa}x{ha} {wb}x{hb}");
}

#[test]
fn a_window_at_150_percent_legible_matches_its_golden() {
    let theme = appearance(
        "theme = dark
text_size = 150
legible = on",
    );
    golden("dark-150-legible-1x", &render_in(&theme, 120).0);
}

/// A colour the person chose over the theme changes the pixels of an app's
/// window, and nothing else of the paint.
#[test]
fn a_token_override_changes_an_app_windows_pixels() {
    let plain = appearance("theme = dark");
    let tinted = appearance(
        "theme = dark
token = text #EEEEEE",
    );
    assert_ne!(
        plain.colour(look::Token::Text),
        tinted.colour(look::Token::Text)
    );
    let (a, fa) = render_in(&plain, 120);
    let (b, fb) = render_in(&tinted, 120);
    // The same layout: only the colours moved.
    assert_eq!(fa.list, fb.list);
    assert_ne!(a.px, b.px, "the text's pixels take the colour chosen");
    assert_eq!(at(&a, 0, 0), at(&b, 0, 0), "the background is the theme's");
}

/// What the layout measures is what the paint draws, in every shipped theme and
/// with text size and legible type on: the row of a line is the line of the
/// style the painter uses. The legible theme's styles are one px larger than the
/// default's, so a measure built from another theme comes out short.
#[test]
fn measured_rows_are_the_painted_rows_in_the_legible_theme_and_the_rest() {
    use uikit::style::TypeStyle as S;
    const STYLES: [S; 7] = [
        S::Small,
        S::Body,
        S::Label,
        S::Heading,
        S::Display,
        S::Mono,
        S::MonoTitle,
    ];
    let others = [
        "theme = dark",
        "theme = light",
        "theme = high-contrast",
        "theme = dark
text_size = 150
legible = on",
    ];
    let legible = appearance("theme = legible");
    let default = appearance("theme = dark");
    for theme in others
        .iter()
        .map(|s| appearance(s))
        .chain([legible.clone()])
    {
        let mut t = text(theme.palette().dark);
        t.set_theme(&theme);
        for style in STYLES {
            let drawn = t.style(crate::look_style(style)).line;
            let (_, measured) = Measured(&t).measure(style, "x");
            assert_eq!(measured, drawn, "{} {style:?}", theme.name);
        }
    }
    // Why it matters: the default's measure is shorter than the legible theme's
    // paint for every style.
    let mut wrong = text(true);
    wrong.set_theme(&default);
    let mut right = text(false);
    right.set_theme(&legible);
    let body = uikit::style::TypeStyle::Body;
    assert!(Measured(&wrong).measure(body, "x").1 < Measured(&right).measure(body, "x").1);
}
