//! The look, pinned: scenes painted on the host, as digests. With
//! `DESK_PREVIEW=dir` each scene is also written there as a PPM to look at.

use crate::desk::Desk;
use crate::geom::{Rect, Size};
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
    mem_used_mib: 412,
    utc_s: None,
    zone_min: 0,
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
        start: None,
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
/// its RGB bytes hashed with the tray blanked. The boot's screendump
/// must hash the same.
const DESK_BOOT: &str = "e2d10823399090a959e01d43604e45e6603c6b55eb5054e67a29095241a917a8";

#[test]
fn the_boot_desk_matches_desk_boot_s_golden() {
    let (w, h) = (1280, 800);
    let mut desk = Desk::new(w, h);
    // As `desktop` boots: the image's own apps pinned on the taskbar.
    let image_apps = crate::taskbar::IMAGE_APPS
        .iter()
        .map(|a| (*a).into())
        .collect();
    desk.set_installed(image_apps);
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
        start: None,
        ask: None,
        now_ms: 0,
    };
    painter.paint(&mut canvas, all, &scene, &mut |_, _, _| false);
    painter.cursor(&mut canvas, all, desk.pointer());
    let stats = crate::paint::tray_area(&desk);
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
        start: None,
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
        "33a1c6d7e7257d26766e11d99f29bf50ae7decc82e99aa82757796a966e0d3de",
    ),
    (
        "light",
        "96cf7f1aa8c792fa3592e8601f2c37707f64e0ce3f2909cced98e235e25c349b",
    ),
    (
        "high-contrast",
        "888683c1c28c79f763ce2a0d3859ba41a2e93bd22f88f72b26bdf2547ef023e3",
    ),
    (
        "legible",
        "8f766389c0c49025a049bde44deb9eadeb7c886e8e61be6f55c2a2ad11234c02",
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
