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
        preview: None,
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

/// `desk-boot`'s golden (xtask/src/cases/kernel/desk.rs): the empty desk at
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
        preview: None,
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
        preview: None,
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

/// `scene` painted whole at `w` x `h`, each window's content as `content`
/// paints it, then the arrow, as `desktop` draws a frame.
fn painted(
    painter: &mut Painter,
    scene: &Scene<'_>,
    w: i32,
    h: i32,
    content: &mut dyn FnMut(crate::desk::WindowId, &mut gfx2d::Canvas<'_>, Rect) -> bool,
) -> Image {
    let mut image = Image::new(w, h);
    let mut canvas = image.canvas();
    let all = Rect::new(0, 0, w, h);
    painter.paint(&mut canvas, all, scene, content);
    painter.cursor(&mut canvas, all, scene.desk.pointer());
    image
}

fn scene_of<'a>(desk: &'a Desk, panel: &'a Panel, chat: &'a crate::chat::Chat) -> Scene<'a> {
    Scene {
        desk,
        bar: &BAR,
        panel,
        chat,
        start: None,
        ask: None,
        preview: None,
        now_ms: 3_000,
    }
}

/// Bug-hunt 5 H5-20: the agent's outline is drawn over every window, so one
/// stacked above the agent's window never covers it: neither one the
/// person raised nor one a background app opened over it unasked.
#[test]
fn the_agents_outline_shows_over_a_window_stacked_above_its_own() {
    let (w, h) = (1280, 800);
    for activated in [true, false] {
        let mut desk = Desk::new(w, h);
        let (agent, _) = desk.open(1, "notes", "Notes", Size { w: 300, h: 200 }, true);
        desk.set_agent(Some(agent));
        let size = Size { w: 300, h: 200 };
        let (above, _) = desk.open(2, "shell", "Terminal", size, activated);
        assert_eq!(desk.windows().last().map(|w| w.id), Some(above));
        let (panel, chat) = (Panel::new(), crate::chat::Chat::default());
        let mut painter = Painter::new(FONTS).expect("the fonts parse");
        let scene = scene_of(&desk, &panel, &chat);
        let image = painted(&mut painter, &scene, w, h, &mut |_, _, _| false);
        let ring = crate::trusted::outline(desk.window(agent).unwrap().frame);
        let inner = ring.grow(-crate::trusted::OUTLINE);
        let top = desk.window(above).unwrap().frame;
        let ai = (0xFF << 24) | painter.palette().ai;
        let mut under = 0;
        for y in ring.y..ring.bottom() {
            for x in ring.x..ring.right() {
                let p = crate::geom::Point { x, y };
                if inner.contains(p) || !top.contains(p) {
                    continue;
                }
                under += 1;
                let got = image.px[(y * w + x) as usize];
                assert_eq!(got, ai, "{x},{y} activated: {activated}");
            }
        }
        // The other window lies over the ring's right and bottom edges.
        assert!(under > 100, "only {under} px of the ring under it");
    }
}

/// What a client drawing at the screen's scale puts on the screen in the
/// tests below: opaque pure green, no colour the desk paints (written so
/// that look's colour-literal lint does not take it for a token).
const MARKER: u32 = (255 << 24) | (255 << 8);
/// Those tests' screen: 1600x900 at 1.25x, a 1280x720 desk.
const PW: i32 = 1600;
const PH: i32 = 900;
const SCALE: u32 = 150;

/// `scene`, a 1280x720 desk, on a [`PW`] x [`PH`] screen as `desktop` shows
/// it, each window drawn at the screen's scale: its content in the logical
/// frame is the hole and the mask names it there, what the painter drew
/// over the windows is no one's ([`Painter::covered`]), and after the
/// resample each window's own px, all [`MARKER`], go where the overlay lets
/// them.
fn shown_sharp(painter: &mut Painter, scene: &Scene<'_>) -> Vec<u32> {
    use crate::{overlay, upscale};
    let (w, h) = (1280, 720);
    let desk = scene.desk;
    let hole = painter.content_fallback() ^ 1;
    let mut mask = alloc::vec![0u32; (w * h) as usize];
    let all = Rect::new(0, 0, w, h);
    let image = {
        let mask = &mut mask;
        let mut content = |id: crate::desk::WindowId, layer: &mut gfx2d::Canvas<'_>, at: Rect| {
            layer.fill(gfx2d::Rect::new(at.x, at.y, at.w, at.h), hole);
            if let Some(area) = desk.window(id).unwrap().content().intersect(&all) {
                overlay::mark(mask, w, h, area, id.0);
            }
            true
        };
        painted(painter, scene, w, h, &mut content)
    };
    for &r in painter.covered() {
        overlay::mark(&mut mask, w, h, r, 0);
    }
    let src = upscale::Source {
        px: &image.px,
        width: w,
        height: h,
    };
    let mut screen = alloc::vec![0u32; (PW * PH) as usize];
    let mut dst = upscale::Target {
        px: &mut screen,
        stride: PW as usize,
        width: PW,
        height: PH,
    };
    let rect = Rect::new(0, 0, PW, PH);
    upscale::resample(&src, &mut dst, SCALE, rect);
    let own = alloc::vec![MARKER; (PW * PH) as usize];
    let phys = |v: i32| (v * SCALE as i32 + 60).div_euclid(120);
    for window in desk.windows() {
        let content = window.content();
        let owner = overlay::Owner {
            mask: &mask,
            id: window.id.0,
            content,
            shade: painter.shade(),
        };
        let place = overlay::Place {
            scale: SCALE,
            rect,
            origin: (phys(content.x), phys(content.y)),
        };
        let buffer = overlay::Buffer {
            px: &own,
            width: PW,
            height: PH,
            stride: PW as usize,
            alpha: false,
        };
        overlay::overlay(&src, hole, &owner, &place, &mut dst, &buffer);
    }
    screen
}

/// Of the physical px on `screen` whose logical px (the one a physical px
/// is the centre of, as the overlay reads it) `inside` holds: how many, and
/// how many show [`MARKER`].
fn markers_in(screen: &[u32], inside: &dyn Fn(crate::geom::Point) -> bool) -> (usize, usize) {
    let centre = |d: i32, len: i32| ((2 * d + 1) * 120 / (2 * SCALE as i32)).min(len - 1);
    let (mut px, mut marked) = (0, 0);
    for y in 0..PH {
        for x in 0..PW {
            let at = crate::geom::Point {
                x: centre(x, 1280),
                y: centre(y, 720),
            };
            if inside(at) {
                px += 1;
                marked += usize::from(screen[(y * PW + x) as usize] == MARKER);
            }
        }
    }
    (px, marked)
}

/// Bug-hunt 5 H5-21: at 1.25x a client drawing at the screen's scale under
/// the ask card puts none of its px on the card, even with the card's
/// ground (`raised`) set to the hole's own colour, the window colour with
/// its blue bit flipped. Its own content round the card still shows.
#[test]
fn a_sharp_client_under_the_card_never_lands_on_it_whatever_its_colour() {
    let (w, h) = (1280, 720);
    let dark = look::theme::Theme::from_palette(&look::DARK, false);
    let theme = dark.with_colour(look::Token::Raised, look::DARK.window ^ 1);
    let mut painter = Painter::new(FONTS).expect("the fonts parse");
    painter.set_theme(&theme);
    let mut desk = Desk::new(w, h);
    let (window, _) = desk.open(1, "app", "App", Size { w, h }, true);
    let card = crate::askcard::Card {
        id: 41,
        call: "file_delete(\"notes/plan.txt\")".into(),
        reason: "The plan is done.".into(),
        shown_ms: 0,
    };
    desk.show_ask(card.id, painter.card_lines(w, &card));
    let (panel, chat) = (Panel::new(), crate::chat::Chat::default());
    let mut scene = scene_of(&desk, &panel, &chat);
    scene.ask = Some(&card);
    let area = crate::trusted::card(w, h, &desk.ask().unwrap());
    let content = desk.window(window).unwrap().content();
    assert_eq!(
        content.intersect(&area),
        Some(area),
        "the window is under the card"
    );
    let screen = shown_sharp(&mut painter, &scene);
    let (px, marked) = markers_in(&screen, &|p| area.contains(p));
    assert!(px > 100_000, "{px} px");
    assert_eq!(marked, 0, "{marked} of the card's {px} px are the client's");
    // Above the card, the client's own px show, but under the arrow.
    let arrow = crate::paint::cursor_box(desk.pointer());
    let above = |p: crate::geom::Point| {
        content.contains(p) && p.y < area.y - 8 && !arrow.grow(1).contains(p)
    };
    let (px, marked) = markers_in(&screen, &above);
    assert!(px > 0 && marked == px, "{marked} of {px} px above the card");
}

/// Bug-hunt 6 H6-19: at 1.25x the focused window's shadow over a lower
/// client's content darkens that client's own px, as it does at 1x, instead of
/// showing the darkened key colour.
#[test]
fn a_shadow_over_a_sharp_client_darkens_its_own_px() {
    let (w, h) = (1280, 720);
    let mut painter = Painter::new(FONTS).expect("the fonts parse");
    let mut desk = Desk::new(w, h);
    let size = Size { w: 600, h: 400 };
    let (lower, _) = desk.open(1, "app", "App", size, true);
    let (upper, _) = desk.open(2, "app", "Other", size, true);
    assert_eq!(desk.focus(), crate::desk::Focus::Window(upper));
    let (below, above) = (
        desk.window(lower).unwrap(),
        desk.window(upper).unwrap().frame,
    );
    let content = below.content();
    let (panel, chat) = (Panel::new(), crate::chat::Chat::default());
    let scene = scene_of(&desk, &panel, &chat);
    let screen = shown_sharp(&mut painter, &scene);
    let shade = painter.shade();
    let centre = |d: i32, len: i32| ((2 * d + 1) * 120 / (2 * SCALE as i32)).min(len - 1);
    let (mut dark, mut wrong) = (0, 0);
    for y in 0..PH {
        for x in 0..PW {
            let p = crate::geom::Point {
                x: centre(x, w),
                y: centre(y, h),
            };
            let light = shade[(p.y * w + p.x) as usize];
            // The lower window's own px, under the upper one's shadow only.
            if light == 255 || !content.contains(p) || above.contains(p) {
                continue;
            }
            dark += 1;
            let got = screen[(y * PW + x) as usize];
            wrong +=
                usize::from((got >> 8 & 0xFF).abs_diff(u32::from(light)) > 2 || got & 0xFF != 0);
        }
    }
    assert!(dark > 500, "{dark} px are in the shadow");
    assert_eq!(
        wrong, 0,
        "{wrong} of {dark} px are not the client's, darkened"
    );
}

/// H5-21's pane and taskbar: `shell` set to the hole's colour, a client
/// drawing at the screen's scale under the open Kai pane and the taskbar
/// shows on neither (the pane's rounded corners aside, which this leaves
/// out: [`Painter::cover`]'s own test pins them).
#[test]
fn a_sharp_client_under_the_pane_or_the_taskbar_never_lands_on_them() {
    let (w, h) = (1280, 720);
    let dark = look::theme::Theme::from_palette(&look::DARK, false);
    let theme = dark.with_colour(look::Token::Shell, look::DARK.window ^ 1);
    let mut painter = Painter::new(FONTS).expect("the fonts parse");
    painter.set_theme(&theme);
    let mut desk = Desk::new(w, h);
    let (window, _) = desk.open(1, "app", "App", Size { w, h }, true);
    let mut effects = Vec::new();
    desk.set_focus(crate::desk::Focus::Panel, &mut effects);
    let pane = desk.pane().expect("the pane is open");
    let content = desk.window(window).unwrap().content();
    let under = content
        .intersect(&pane)
        .expect("the window is under the pane");
    assert_eq!(
        (under.x, under.w, under.bottom()),
        (pane.x, pane.w, pane.bottom())
    );
    let bar = desk
        .layout()
        .taskbar
        .intersect(&content)
        .expect("and under the taskbar");
    let (panel, chat) = (Panel::new(), crate::chat::Chat::default());
    let scene = scene_of(&desk, &panel, &chat);
    let screen = shown_sharp(&mut painter, &scene);
    let r = look::metric::RADIUS_MD as i32;
    let on_pane = |p: crate::geom::Point| {
        let across = p.x >= pane.x + r && p.x < pane.right() - r;
        let down = p.y >= pane.y + r && p.y < pane.bottom() - r;
        under.contains(p) && (across || down)
    };
    let (px, marked) = markers_in(&screen, &on_pane);
    assert!(px > 100_000, "{px} px");
    assert_eq!(marked, 0, "{marked} of the pane's {px} px are the client's");
    let (px, marked) = markers_in(&screen, &|p| bar.contains(p));
    assert!(px > 10_000, "{px} px");
    assert_eq!(
        marked, 0,
        "{marked} of the taskbar's {px} px are the client's"
    );
}

/// [`Painter::cover`] notes every px of a layer rounded as
/// [`gfx2d::Image::round_corners`] rounds it that the layer reaches at all,
/// once, and nothing else.
#[test]
fn a_rounded_cover_is_every_px_the_rounded_layer_reaches() {
    let shapes = [(40, 30, 8), (9, 7, 4), (16, 16, 8), (5, 40, 9), (30, 20, 0)];
    for (lw, lh, radius) in shapes {
        let mut layer = Image::new(lw, lh);
        layer
            .canvas()
            .fill(gfx2d::Rect::new(0, 0, lw, lh), u32::MAX);
        layer.round_corners(radius);
        let mut painter = Painter::new(FONTS).expect("the fonts parse");
        let at = Rect::new(3, 5, lw, lh);
        painter.cover(at, radius, Rect::new(0, 0, 100, 100));
        for y in 0..lh {
            for x in 0..lw {
                let reached = layer.px[(y * lw + x) as usize] >> 24 > 0;
                let p = crate::geom::Point {
                    x: at.x + x,
                    y: at.y + y,
                };
                let covered = painter.covered().iter().filter(|r| r.contains(p)).count();
                assert_eq!(
                    covered,
                    usize::from(reached),
                    "{x},{y} of {lw}x{lh} r{radius}"
                );
            }
        }
    }
}

/// The arrow's own px join [`Painter::covered`], once each, and the rest of
/// its box does not: a sharp client under the cursor still shows round it.
#[test]
fn the_arrow_covers_its_own_px_and_nothing_round_it() {
    let mut painter = Painter::new(FONTS).expect("the fonts parse");
    let mut image = Image::new(40, 40);
    let at = crate::geom::Point { x: 7, y: 5 };
    painter.cursor(&mut image.canvas(), Rect::new(0, 0, 40, 40), at);
    let palette = painter.palette();
    for y in 0..40 {
        for x in 0..40 {
            let p = crate::geom::Point { x, y };
            let drawn = crate::paint::arrow(x - at.x, y - at.y, palette).is_some();
            let covered = painter.covered().iter().filter(|r| r.contains(p)).count();
            assert_eq!(covered, usize::from(drawn), "{x},{y}");
        }
    }
}

/// With no client drawn at the screen's scale the shadows' light is not kept:
/// the frame is the same px, and the buffer is not written.
#[test]
fn the_frame_is_the_same_when_the_shadows_light_is_not_kept() {
    let (w, h) = (1280, 720);
    let mut desk = Desk::new(w, h);
    let size = Size { w: 600, h: 400 };
    desk.open(1, "app", "App", size, true);
    desk.open(2, "app", "Other", size, true);
    let (panel, chat) = (Panel::new(), crate::chat::Chat::default());
    let scene = scene_of(&desk, &panel, &chat);
    let mut frames = Vec::new();
    for tracked in [true, false] {
        let mut painter = Painter::new(FONTS).expect("the fonts parse");
        painter.track_shade(tracked);
        let mut content = |_: crate::desk::WindowId, _: &mut gfx2d::Canvas<'_>, _: Rect| true;
        let image = painted(&mut painter, &scene, w, h, &mut content);
        assert_eq!(
            painter.shade().iter().any(|&v| v < 255),
            tracked,
            "tracked: {tracked}"
        );
        frames.push(image.px);
    }
    assert!(frames[0] == frames[1], "the frames differ");
}
