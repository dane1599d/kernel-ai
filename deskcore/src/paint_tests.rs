//! The look, pinned: scenes painted on the host, as digests. With
//! `DESK_PREVIEW=dir` each scene is also written there as a PPM to look at.

use crate::desk::Desk;
use crate::geom::{Point, Rect, Size};
use crate::paint::{Bar, Painter, Scene};
use crate::panel::Panel;
use alloc::string::String;
use alloc::vec::Vec;
use gfx2d::{Fonts, Image};
use sha2::{Digest, Sha256};

pub(crate) const FONTS: Fonts = Fonts {
    sans: include_bytes!("../../assets/fonts/InterVariable.ttf"),
    mono: include_bytes!("../../assets/fonts/JetBrainsMono-Variable.ttf"),
};

pub(crate) const BAR: Bar = Bar {
    model: true,
    warming: false,
    mem_used_mib: 412,
    mem_total_mib: 1763,
};

fn paint(name: &str, desk: &Desk, w: i32, h: i32) -> String {
    paint_with(name, desk, &Panel::new(), w, h)
}

fn paint_with(name: &str, desk: &Desk, panel: &Panel, w: i32, h: i32) -> String {
    let painter = Painter::new(FONTS).expect("the fonts parse");
    paint_in(painter, name, desk, panel, w, h)
}

fn paint_in(
    mut painter: Painter,
    name: &str,
    desk: &Desk,
    panel: &Panel,
    w: i32,
    h: i32,
) -> String {
    let mut image = Image::new(w, h);
    let mut canvas = image.canvas();
    let scene = Scene {
        desk,
        bar: &BAR,
        panel,
        chat: &crate::chat::Chat::default(),
        sheet: None,
        ask: None,
        now_ms: 4_200,
    };
    painter.paint(
        &mut canvas,
        Rect::new(0, 0, w, h),
        &scene,
        &mut |_, _, _| false,
    );
    painter.cursor(&mut canvas, Rect::new(0, 0, w, h), desk.pointer());
    if let Ok(dir) = std::env::var("DESK_PREVIEW") {
        let mut ppm = std::format!("P6\n{w} {h}\n255\n").into_bytes();
        for p in &image.px {
            ppm.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
        }
        std::fs::write(std::format!("{dir}/{name}.ppm"), ppm).expect("the preview is written");
    }
    let bytes: Vec<u8> = image.px.iter().flat_map(|p| p.to_le_bytes()).collect();
    Sha256::digest(&bytes)
        .iter()
        .map(|b| std::format!("{b:02x}"))
        .collect()
}

#[test]
fn an_empty_desk_paints_the_same_every_time() {
    let desk = Desk::new(1280, 800);
    let first = paint("empty", &desk, 1280, 800);
    assert_eq!(first, paint("empty", &desk, 1280, 800));
}

#[test]
fn two_windows_paint() {
    let mut desk = Desk::new(1280, 800);
    desk.open(1, "notes", "Notes", Size { w: 400, h: 300 }, true);
    desk.open(2, "shell", "Terminal", Size { w: 520, h: 320 }, true);
    paint("two", &desk, 1280, 800);
}

/// `desk-boot`'s golden (xtask/src/cases/kernel.rs): the empty desk at
/// 1280x800 with the cursor in the middle, as `desktop` first draws it on GOP,
/// its RGB bytes hashed with the top bar's stats blanked. The boot's screendump
/// must hash the same.
const DESK_BOOT: &str = "5409c90f0bc6830ab1ed54e061ab040b69b3aa28e68e87b9d73549bb94ce6b32";

#[test]
fn the_boot_desk_matches_desk_boot_s_golden() {
    let (w, h) = (1280, 800);
    let desk = Desk::new(w, h);
    let mut painter = Painter::new(FONTS).expect("the fonts parse");
    let mut image = Image::new(w, h);
    let mut canvas = image.canvas();
    let all = Rect::new(0, 0, w, h);
    let panel = Panel::new();
    let scene = Scene {
        desk: &desk,
        bar: &BAR,
        panel: &panel,
        chat: &crate::chat::Chat::default(),
        sheet: None,
        ask: None,
        now_ms: 0,
    };
    painter.paint(&mut canvas, all, &scene, &mut |_, _, _| false);
    painter.cursor(&mut canvas, all, desk.pointer());
    let stats = crate::paint::stats_area(w);
    let mut rgb = Vec::new();
    for (i, p) in image.px.iter().enumerate() {
        let (x, y) = (i as i32 % w, i as i32 / w);
        let masked = x >= stats.x && x < stats.right() && y >= stats.y && y < stats.bottom();
        let p = if masked { 0 } else { *p };
        rgb.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
    }
    let got: String = Sha256::digest(&rgb)
        .iter()
        .map(|b| std::format!("{b:02x}"))
        .collect();
    assert_eq!(
        got, DESK_BOOT,
        "desk-boot's golden changed: update it here and in xtask"
    );
}

#[test]
fn an_ask_card_paints_above_a_window() {
    let mut desk = Desk::new(1280, 800);
    desk.open(1, "notes", "Notes", Size { w: 400, h: 300 }, true);
    let card = crate::askcard::Card {
        id: 41,
        call: "process_stop(\"build-3\")".into(),
        reason: "build-3 has used a whole CPU for ten minutes and the editor lags.".into(),
        shown_ms: 0,
    };
    let painter = Painter::new(FONTS).expect("the fonts parse");
    let lines = painter.card_lines(1280, &card);
    desk.show_ask(card.id, lines);
    let (w, h) = (1280, 800);
    let mut painter = painter;
    let mut image = Image::new(w, h);
    let mut canvas = image.canvas();
    let panel = Panel::new();
    let scene = Scene {
        desk: &desk,
        bar: &BAR,
        panel: &panel,
        chat: &crate::chat::Chat::default(),
        sheet: None,
        ask: Some(&card),
        now_ms: 3_000,
    };
    painter.paint(
        &mut canvas,
        Rect::new(0, 0, w, h),
        &scene,
        &mut |_, _, _| false,
    );
    if let Ok(dir) = std::env::var("DESK_PREVIEW") {
        let mut ppm = std::format!(
            "P6
{w} {h}
255
"
        )
        .into_bytes();
        for p in &image.px {
            ppm.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
        }
        std::fs::write(std::format!("{dir}/ask.ppm"), ppm).expect("the preview is written");
    }
}

/// Each shipped theme's golden: two windows, the second focused (with its
/// shadow), and the Kai pane open beside them with its input listening, at
/// 1280x800. Its digest is pinned, so a
/// change to a theme or to the painter shows here; `DESK_PREVIEW=dir` writes
/// each as a PPM to look at.
const THEME_GOLDENS: [(&str, &str); 4] = [
    (
        "dark",
        "9b941e791223f71fa5170883bfc9a9484457e04f3f368fa3b367de81bbccf798",
    ),
    (
        "light",
        "120604d298eec2d0ebf787d7a94406dc1ce6dc2ad41eeafa4c8bb4defd0a11c3",
    ),
    (
        "high-contrast",
        "a6834c8c1e2bd4eb2018f7afaa863c5782e75e5ba3401ee0d75e40a4d9e75d8b",
    ),
    (
        "legible",
        "69e6b9671f6a783dc20a579fe5d8b0e9d24298059ccc7d6009e6852d75e77c4b",
    ),
];

#[test]
fn each_shipped_theme_paints_its_golden() {
    let mut desk = Desk::new(1280, 800);
    desk.open(1, "notes", "Notes", Size { w: 400, h: 300 }, true);
    let (top, _) = desk.open(2, "shell", "Terminal", Size { w: 520, h: 320 }, true);
    // Opened, then the focus back on the window: at 1280 the pane stays.
    let mut effects = Vec::new();
    desk.set_focus(crate::desk::Focus::Panel, &mut effects);
    desk.set_focus(crate::desk::Focus::Window(top), &mut effects);
    assert!(desk.pane().is_some());
    let mut panel = Panel::new();
    panel.set_focus(true);
    let mut seen: Vec<String> = Vec::new();
    for theme in look::theme::shipped() {
        let mut painter = Painter::new(FONTS).expect("the fonts parse");
        painter.set_theme(&theme);
        let name = std::format!("theme-{}", theme.name);
        let got = paint_in(painter, &name, &desk, &panel, 1280, 800);
        let want = THEME_GOLDENS
            .iter()
            .find(|(n, _)| *n == theme.name)
            .map(|(_, sha)| *sha);
        assert_eq!(Some(got.as_str()), want, "{}'s golden changed", theme.name);
        assert!(
            !seen.contains(&got),
            "{} paints like another theme",
            theme.name
        );
        seen.push(got);
    }
}

/// The dark theme read from its file paints what the built-in dark palette
/// paints: `set_theme` changes nothing until the theme does.
#[test]
fn the_dark_theme_paints_the_default_desk() {
    let desk = Desk::new(1280, 800);
    let panel = Panel::new();
    let default = paint_with("default", &desk, &panel, 1280, 800);
    let mut painter = Painter::new(FONTS).expect("the fonts parse");
    let dark = look::theme::Theme::parse(&look::theme::shipped()[0].encode()).unwrap();
    painter.set_theme(&dark);
    assert_eq!(paint_in(painter, "dark", &desk, &panel, 1280, 800), default);
}

#[test]
fn a_dragged_window_has_no_shadow_until_it_is_let_go() {
    use crate::desk::{Button, Input};
    use crate::geom::Point;
    let mut desk = Desk::new(1280, 800);
    let (id, _) = desk.open(1, "notes", "Notes", Size { w: 400, h: 300 }, true);
    let frame = desk.windows()[0].frame;
    desk.input(Input::Motion(Point {
        x: frame.x + 60,
        y: frame.y + 10,
    }));
    let shadowed = paint("shadowed", &desk, 1280, 800);
    let press = |desk: &mut Desk, down: bool| {
        desk.input(Input::Button {
            button: Button::Left,
            down,
        })
    };
    press(&mut desk, true);
    assert_eq!(desk.dragged(), Some(id));
    assert_ne!(paint("dragged", &desk, 1280, 800), shadowed);
    press(&mut desk, false);
    assert_eq!(paint("let-go", &desk, 1280, 800), shadowed);
}

#[test]
fn the_input_view_keeps_the_cursor_in_its_columns() {
    use crate::paint_panel::view;
    // A line that fits, the cursor anywhere in it: nothing is skipped.
    assert_eq!(view(10, 10, 20), 0);
    assert_eq!(view(30, 5, 20), 0);
    assert_eq!(view(30, 19, 20), 0);
    // The cursor past the last column: the marker takes column 0 and the
    // cursor sits in the last column, 19.
    assert_eq!(view(30, 20, 20), 2);
    assert_eq!(view(30, 30, 20), 12);
    assert_eq!(30 - view(30, 30, 20) + 1, 19);
    // A cursor past the end is the end; an input too narrow skips nothing.
    assert_eq!(view(30, 99, 20), 12);
    assert_eq!(view(30, 30, 1), 0);
}

/// Kai Desk PR 3: the Apps sheet is the scene's, drawn under the ask card,
/// so a card over an open sheet shows exactly as it does alone.
#[test]
fn a_card_paints_over_an_open_sheet() {
    use crate::catalog::{Sheet, Source};
    let (w, h) = (1280, 800);
    let mut desk = Desk::new(w, h);
    let card = crate::askcard::Card {
        id: 41,
        call: "process_stop(\"build-3\")".into(),
        reason: "build-3 has used a whole CPU for ten minutes.".into(),
        shown_ms: 0,
    };
    let lines = Painter::new(FONTS)
        .expect("the fonts parse")
        .card_lines(w, &card);
    desk.show_ask(card.id, lines);
    let rows = (0..8)
        .map(|i| {
            crate::catalog::tests::app(
                Source::Image,
                &std::format!("app{i}"),
                None,
                "Run as Kai asks: press y twice.",
            )
        })
        .collect();
    let sheet = Sheet::new(rows);
    let panel = Panel::new();
    let draw = |sheet: Option<&Sheet>| {
        let mut painter = Painter::new(FONTS).expect("the fonts parse");
        let mut image = Image::new(w, h);
        let scene = Scene {
            desk: &desk,
            bar: &BAR,
            panel: &panel,
            chat: &crate::chat::Chat::default(),
            sheet,
            ask: Some(&card),
            now_ms: 3_000,
        };
        painter.paint(
            &mut image.canvas(),
            Rect::new(0, 0, w, h),
            &scene,
            &mut |_, _, _| false,
        );
        image
    };
    let (with, alone) = (draw(Some(&sheet)), draw(None));
    let area = crate::paint_catalog::sheet(w, h, sheet.apps().len());
    let on_card = crate::trusted::card(w, h, &desk.ask().unwrap());
    let both = area
        .intersect(&on_card)
        .expect("the sheet is under the card");
    assert!(both.w * both.h > 1000, "{both:?}");
    for y in on_card.y..on_card.y + on_card.h {
        for x in on_card.x..on_card.x + on_card.w {
            let i = (y * w + x) as usize;
            assert_eq!(with.px[i], alone.px[i], "({x}, {y})");
        }
    }
    // The sheet is drawn where the card is not.
    let outside = (area.y..area.y + area.h)
        .flat_map(|y| (area.x..area.x + area.w).map(move |x| (x, y)))
        .filter(|&(x, y)| !on_card.contains(Point { x, y }))
        .any(|(x, y)| with.px[(y * w + x) as usize] != alone.px[(y * w + x) as usize]);
    assert!(outside, "the sheet was not drawn");
}

/// Kai Desk PR 3: a window with no pixels yet shows the theme's window
/// colour, not the dark one's.
#[test]
fn the_light_themes_fallback_content_is_the_light_window_colour() {
    let mut painter = Painter::new(FONTS).expect("the fonts parse");
    assert_eq!(painter.content_fallback() & 0xFF_FFFF, look::DARK.window);
    let light = look::theme::Theme::from_palette(&look::LIGHT, false);
    painter.set_theme(&light);
    assert_eq!(
        painter.content_fallback(),
        (0xFF << 24) | look::LIGHT.window
    );
}
