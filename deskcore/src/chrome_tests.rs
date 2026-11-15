//! Kai Desk's window chrome (PR 5): the title row's label and title, the
//! controls' hover, and a maximized window's corners and grabs.

use crate::chrome::{self, Part};
use crate::desk::{Button, Desk, Effect, Hit, Input, WindowId};
use crate::geom::{Point, Rect, Size};
use crate::paint::{PAD, Painter, Scene};
use crate::paint_tests::{BAR, FONTS};
use crate::panel::Panel;
use alloc::vec::Vec;
use gfx2d::Image;
use look::theme::{self, Theme};
use look::{Palette, Token};

const W: i32 = 1280;
const H: i32 = 800;
/// The colour bits of a pixel, its alpha byte dropped.
const RGB: u32 = (1 << 24) - 1;
/// Where the kernel's label starts in a title bar: after the pad, the 16 px
/// icon and the 8 px gap.
const LABEL_X: i32 = PAD + 16 + 8;

/// The desk painted whole, in `theme` (dark when none).
fn painted_in(desk: &Desk, theme: Option<&Theme>) -> Image {
    let mut painter = Painter::new(FONTS).expect("the fonts parse");
    if let Some(theme) = theme {
        painter.set_theme(theme);
    }
    let mut image = Image::new(W, H);
    let mut canvas = image.canvas();
    let scene = Scene {
        desk,
        bar: &BAR,
        panel: &Panel::new(),
        chat: &crate::chat::Chat::default(),
        start: None,
        ask: None,
        now_ms: 0,
    };
    painter.paint(
        &mut canvas,
        Rect::new(0, 0, W, H),
        &scene,
        &mut |_, _, _| false,
    );
    image
}

fn painted(desk: &Desk) -> Image {
    painted_in(desk, None)
}

/// The pixels of `r` in `image`, row by row.
fn pixels(image: &Image, r: Rect) -> Vec<u32> {
    (r.y..r.bottom())
        .flat_map(|y| (r.x..r.right()).map(move |x| (x, y)))
        .map(|(x, y)| px(image, x, y))
        .collect()
}

fn px(image: &Image, x: i32, y: i32) -> u32 {
    image.px[(y * W + x) as usize] & RGB
}

/// One window of `label` titled `title`, `w` px of content wide, focused.
fn sized(label: &str, title: &str, w: i32) -> (Desk, WindowId) {
    let mut desk = Desk::new(W, H);
    let (id, _) = desk.open(1, label, title, Size { w, h: 300 }, true);
    (desk, id)
}

fn one(label: &str, title: &str) -> (Desk, WindowId) {
    sized(label, title, 400)
}

/// The title bar's text, from the icon to the first control.
fn text_area(desk: &Desk, id: WindowId) -> Rect {
    let frame = desk.window(id).expect("open").frame;
    let bar = chrome::title_bar(frame);
    let end = chrome::controls(frame)[2].1.x;
    Rect::new(bar.x, bar.y, end - bar.x, bar.h)
}

/// One past the last column of `area` where `image` differs from `blank`.
fn ink_end(image: &Image, blank: &Image, area: Rect) -> i32 {
    (area.x..area.right())
        .filter(|&x| (area.y..area.bottom()).any(|y| px(image, x, y) != px(blank, x, y)))
        .max()
        .map_or(area.x, |x| x + 1)
}

fn middle(r: Rect) -> Point {
    Point {
        x: r.x + r.w / 2,
        y: r.y + r.h / 2,
    }
}

fn click(desk: &mut Desk) -> Vec<Effect> {
    let mut effects = Vec::new();
    for down in [true, false] {
        effects.extend(desk.input(Input::Button {
            button: Button::Left,
            down,
        }));
    }
    effects
}

/// Clicks `id`'s maximize (or restore) control, and commits the size the
/// desk asks for, as a client does.
fn toggle_maximize(desk: &mut Desk, id: WindowId) {
    let frame = desk.window(id).expect("open").frame;
    desk.input(Input::Motion(middle(chrome::controls(frame)[1].1)));
    for effect in click(desk) {
        if let Effect::Configure { window, size, .. } = effect {
            desk.commit(window, size, &[]);
        }
    }
}

/// ADR 0005 §7: an app cannot pose as another by its title. Two windows with
/// the same icon (a monogram of the same letter) and title, under other
/// labels: their label spans differ, and each is the label as drawn alone.
#[test]
fn an_app_titled_like_another_still_shows_its_label() {
    let (spam, a) = one("spam", "Notes");
    let (scam, _) = one("scam", "Notes");
    let area = text_area(&spam, a);
    let (spam, scam) = (painted(&spam), painted(&scam));
    let alone = painted(&one("spam", "").0);
    let blank = painted(&one("", "").0);
    let from = area.x + LABEL_X;
    let end = ink_end(&alone, &blank, area);
    assert!(end > from + 8, "the label alone paints nothing");
    let label = Rect::new(from, area.y, end - from, area.h);
    assert_eq!(pixels(&spam, label), pixels(&alone, label));
    assert_ne!(pixels(&spam, label), pixels(&scam, label), "no label drawn");
    let icon = Rect::new(area.x, area.y, LABEL_X, area.h);
    assert_eq!(pixels(&spam, icon), pixels(&scam, icon));
}

/// A long title is cut at the text's end: the bar shows exactly what a wide
/// window shows there (the label, then the title's head), the title reaches
/// the cut, and the controls paint as under a short title.
#[test]
fn the_label_then_the_title_is_cut_from_the_titles_end() {
    let long = "Groceries and more ".repeat(20);
    let (narrow, id) = one("notes", &long);
    let area = text_area(&narrow, id);
    let narrow = painted(&narrow);
    let wide = painted(&sized("notes", &long, 1100).0);
    assert_eq!(pixels(&narrow, area), pixels(&wide, area));
    let blank = painted(&one("notes", "").0);
    let tail = Rect::new(area.right() - 20, area.y, 20, area.h);
    assert_ne!(
        pixels(&narrow, tail),
        pixels(&blank, tail),
        "no title at the cut"
    );
    let short = painted(&one("notes", "N").0);
    let controls = Rect::new(area.right(), area.y, 3 * chrome::CONTROL_W, area.h);
    assert_eq!(pixels(&narrow, controls), pixels(&short, controls));
}

/// A title's marks that reach left (combining marks first) are clipped at
/// the title's start: the label paints as with no title.
#[test]
fn a_title_cannot_draw_over_the_label() {
    let (marked, id) = one("notes", "\u{489}\u{489}\u{489}\u{488}Notes");
    let area = text_area(&marked, id);
    let alone = painted(&one("notes", "").0);
    let blank = painted(&one("", "").0);
    let end = ink_end(&alone, &blank, area);
    let label = Rect::new(area.x, area.y, end - area.x, area.h);
    assert_eq!(pixels(&painted(&marked), label), pixels(&alone, label));
}

/// No pixel of a title shows without the whole label before it, at any
/// width down to the narrowest, where the controls fill the bar.
#[test]
fn a_title_never_shows_without_its_whole_label() {
    const LABEL: &str = "notes-window";
    let (wide, id) = one(LABEL, "");
    let area = text_area(&wide, id);
    let wide = painted(&wide);
    let end = ink_end(&wide, &painted(&one("", "").0), area);
    let label = Rect::new(area.x + LABEL_X, area.y, end - area.x - LABEL_X, area.h);
    let mut shown = 0;
    for w in (0..=400).step_by(10) {
        let (desk, id) = sized(LABEL, "Groceries", w);
        // The window's own title bar only.
        let bar = text_area(&desk, id);
        let titled = painted(&desk);
        let bare = painted(&sized(LABEL, "", w).0);
        if pixels(&titled, bar) == pixels(&bare, bar) {
            continue;
        }
        shown += 1;
        assert_eq!(pixels(&titled, label), pixels(&wide, label), "at {w}");
    }
    assert!(shown > 0, "the title never shows");
    // The narrowest: the three controls fill the bar.
    let (narrow, id) = sized(LABEL, "Groceries", 0);
    assert_eq!(text_area(&narrow, id).w, 0);
}

/// A client cannot commit a window narrower than the desk opens one.
#[test]
fn a_commit_is_never_narrower_than_the_least_window() {
    let mut desk = Desk::new(W, H);
    let (id, _) = desk.open(1, "notes", "Notes", Size { w: 0, h: 0 }, true);
    let least = desk.window(id).expect("open").content();
    desk.commit(id, Size { w: 10, h: 10 }, &[]);
    let content = desk.window(id).expect("open").content();
    assert_eq!((content.w, content.h), (least.w, least.h));
}

/// The pointer over a control hovers it, and damages it; moving off clears
/// the hover and damages it again. Close hovers in `error`.
#[test]
fn a_hovered_control_is_tracked_damaged_and_painted() {
    let (mut desk, id) = one("notes", "Notes");
    let frame = desk.window(id).expect("open").frame;
    let [(_, close), (_, max), _] = chrome::controls(frame);
    let before = painted(&desk);
    let _ = desk.take_damage();
    desk.input(Input::Motion(middle(close)));
    assert_eq!(desk.hovered(), Some((id, Part::Close)));
    assert!(
        desk.take_damage()
            .iter()
            .any(|d| d.intersect(&close).is_some())
    );
    let hovered = painted(&desk);
    let error = look::DARK.error;
    assert!(pixels(&hovered, close).contains(&error), "close is not red");
    assert!(!pixels(&before, close).contains(&error));
    // Over maximize: close goes back as it was, maximize fills with hover.
    desk.input(Input::Motion(middle(max)));
    assert_eq!(desk.hovered(), Some((id, Part::Maximize)));
    let damage = desk.take_damage();
    assert!(damage.iter().any(|d| d.intersect(&close).is_some()));
    assert!(damage.iter().any(|d| d.intersect(&max).is_some()));
    let on_max = painted(&desk);
    assert_eq!(pixels(&on_max, close), pixels(&before, close));
    assert!(pixels(&on_max, max).contains(&look::DARK.hover));
    // Off the bar: nothing hovered.
    desk.input(Input::Motion(Point { x: 5, y: H / 2 }));
    assert_eq!(desk.hovered(), None);
    assert_eq!(pixels(&painted(&desk), max), pixels(&before, max));
}

/// What the pointer is over, as a hover.
fn under_pointer(desk: &Desk) -> Option<(WindowId, Part)> {
    match desk.hit(desk.pointer()) {
        Hit::Window(id, part @ (Part::Minimize | Part::Maximize | Part::Close)) => Some((id, part)),
        _ => None,
    }
}

/// A click that moves the controls (maximize, restore, minimize) or a card
/// over them leaves no hover on what is no longer under the pointer.
#[test]
fn the_hover_follows_what_is_under_the_pointer() {
    let (mut desk, id) = one("notes", "Notes");
    toggle_maximize(&mut desk, id);
    assert!(desk.window(id).expect("open").maximized());
    let _ = desk.take_damage();
    assert_ne!(desk.hovered(), Some((id, Part::Maximize)));
    assert_eq!(desk.hovered(), under_pointer(&desk));
    // Restore, from the maximized window's own control.
    toggle_maximize(&mut desk, id);
    assert!(!desk.window(id).expect("open").maximized());
    let _ = desk.take_damage();
    assert_eq!(desk.hovered(), under_pointer(&desk));
    // A card over a hovered control takes the hover away.
    let frame = desk.window(id).expect("open").frame;
    desk.input(Input::Motion(middle(chrome::controls(frame)[0].1)));
    assert_eq!(desk.hovered(), Some((id, Part::Close)));
    desk.show_ask(1, 1);
    let _ = desk.take_damage();
    assert_eq!(desk.hovered(), None);
    desk.hide_ask(1);
    let _ = desk.take_damage();
    assert_eq!(desk.hovered(), Some((id, Part::Close)));
    // Minimize: the window goes, and its hover with it.
    desk.input(Input::Motion(middle(chrome::controls(frame)[2].1)));
    click(&mut desk);
    assert!(desk.window(id).expect("open").minimized);
    assert_eq!(desk.hovered(), None);
}

/// A maximized window has square corners and no grabs: the pointer pushed
/// against the screen's right edge on the close row is on close, which a
/// click presses.
#[test]
fn a_maximized_window_is_square_and_closes_from_the_right_edge() {
    let (mut desk, id) = one("notes", "Notes");
    let corner = |desk: &Desk| {
        let f = desk.window(id).expect("open").frame;
        px(&painted(desk), f.x, f.y)
    };
    let line = look::DARK.line_strong;
    assert_ne!(corner(&desk), line, "a window's corner is rounded");
    toggle_maximize(&mut desk, id);
    assert!(desk.window(id).expect("open").maximized());
    assert_eq!(corner(&desk), line, "a maximized window's corner is square");
    let frame = desk.window(id).expect("open").frame;
    for y in [frame.y, frame.y + 1, frame.y + chrome::CONTROL_H - 1] {
        let edge = Point {
            x: frame.right() - 1,
            y,
        };
        assert_eq!(desk.hit(edge), Hit::Window(id, Part::Close), "at y {y}");
    }
    desk.input(Input::Motion(Point {
        x: W + 50,
        y: frame.y,
    }));
    let effects = click(&mut desk);
    assert!(effects.contains(&Effect::Close { window: id }));
    assert!(
        desk.window(id).expect("open").maximized(),
        "no resize began"
    );
}

/// How far apart two colours are: the sum of their channels' distances.
fn dist(a: u32, b: u32) -> u32 {
    (0..3)
        .map(|i| ((a >> (8 * i)) & 0xFF).abs_diff((b >> (8 * i)) & 0xFF))
        .sum()
}

/// The farthest pixel of `r` from `hover`: a glyph's ink at full cover.
fn far_from(image: &Image, r: Rect, hover: u32) -> u32 {
    pixels(image, r)
        .into_iter()
        .max_by_key(|&p| dist(p, hover))
        .expect("pixels")
}

/// Asserts the ink on `hover` in `r` is `ink`, never a `HOVER_NEVER` token.
fn assert_ink(image: &Image, r: Rect, palette: &Palette, ink: u32, what: &str) {
    let far = far_from(image, r, palette.hover);
    for token in theme::HOVER_NEVER {
        let never = palette.get(token);
        if never == ink {
            continue;
        }
        assert!(
            dist(far, ink) < dist(far, never),
            "{what}: {token:?} on hover ({far:06x})"
        );
    }
}

/// ADR 0005 §8: `dim`, `ai` and `line_strong` are never drawn on `hover`. In
/// every shipped theme: a monogram's letter, focused or not, and a hovered
/// control's glyph, focused or not, are drawn in their own inks.
#[test]
fn nothing_on_hover_is_drawn_in_a_hover_never_token() {
    assert_eq!(
        theme::HOVER_NEVER,
        [Token::Dim, Token::Ai, Token::LineStrong]
    );
    for theme in theme::shipped() {
        let palette = theme.palette();
        // `spam` has no glyph of ours: a monogram. Unfocused under `notes`.
        let mut desk = Desk::new(W, H);
        let (spam, _) = desk.open(1, "spam", "Spam", Size { w: 400, h: 300 }, true);
        let (bacon, _) = desk.open(2, "bacon", "Bacon", Size { w: 400, h: 300 }, true);
        for (id, focused) in [(spam, false), (bacon, true)] {
            let frame = desk.window(id).expect("open").frame;
            let bar = chrome::title_bar(frame);
            let tile = Rect::new(bar.x + PAD, bar.y + (bar.h - 16) / 2, 16, 16);
            let max = chrome::controls(frame)[1].1;
            desk.input(Input::Motion(middle(max)));
            let image = painted_in(&desk, Some(&theme));
            let what = alloc::format!("{} focused={focused}", theme.name);
            let ink = palette.muted;
            assert_ink(
                &image,
                tile,
                &palette,
                ink,
                &alloc::format!("{what} monogram"),
            );
            let inside = Rect::new(max.x + 1, max.y + 1, max.w - 2, max.h - 2);
            let ink = palette.text;
            assert_ink(
                &image,
                inside,
                &palette,
                ink,
                &alloc::format!("{what} control"),
            );
        }
    }
}

/// High contrast: `hover` is the window's ground, so a hovered control shows
/// a 1 px `focus` frame, not the agent outline's 2 px.
#[test]
fn high_contrast_frames_a_hovered_control_in_one_px() {
    let theme = theme::shipped()
        .into_iter()
        .find(|t| t.name == "high-contrast")
        .expect("shipped");
    let palette = theme.palette();
    assert_eq!(palette.hover, palette.window);
    let (mut desk, id) = one("notes", "Notes");
    let max = chrome::controls(desk.window(id).expect("open").frame)[1].1;
    desk.input(Input::Motion(middle(max)));
    let image = painted_in(&desk, Some(&theme));
    let y = max.y + max.h / 2;
    assert_eq!(px(&image, max.x, y), palette.focus);
    assert_ne!(px(&image, max.x + 1, y), palette.focus);
}

/// A monogram's letter sits whole and centred on its 16 px tile, at 100% to
/// 200% text in every shipped theme: wide (W), narrow (I) and round (S).
#[test]
fn a_monogram_is_centred_and_fits_its_tile_at_any_text_size() {
    for base in theme::shipped() {
        for percent in [100, 150, 200] {
            let theme = base.sized(false, percent);
            let hover = theme.palette().hover;
            for label in ["wiki", "ink", "spam"] {
                let (desk, id) = one(label, "");
                let bar = chrome::title_bar(desk.window(id).expect("open").frame);
                let tile = Rect::new(bar.x + PAD, bar.y + (bar.h - 16) / 2, 16, 16);
                let image = painted_in(&desk, Some(&theme));
                let inked: Vec<(i32, i32)> = (tile.y..tile.bottom())
                    .flat_map(|y| (tile.x..tile.right()).map(move |x| (x, y)))
                    .filter(|&(x, y)| px(&image, x, y) != hover)
                    .collect();
                let what = alloc::format!("{} {percent}% {label}", theme.name);
                let xs = || inked.iter().map(|p| p.0);
                let ys = || inked.iter().map(|p| p.1);
                let (Some(left), Some(right)) = (xs().min(), xs().max()) else {
                    panic!("{what}: no letter");
                };
                let (top, bottom) = (ys().min().unwrap_or(0), ys().max().unwrap_or(0));
                // Whole: clear of the tile's edges, so nothing was cut.
                assert!(
                    left > tile.x && right < tile.right() - 1,
                    "{what}: cut across"
                );
                assert!(
                    top > tile.y && bottom < tile.bottom() - 1,
                    "{what}: cut down"
                );
                // Centred on its ink, within a pixel either way.
                let centre = |a: i32, b: i32, from: i32| (a + b + 1) - 2 * from - 16;
                assert!(
                    centre(left, right, tile.x).abs() <= 2,
                    "{what}: off centre across"
                );
                assert!(
                    centre(top, bottom, tile.y).abs() <= 2,
                    "{what}: off centre down"
                );
            }
        }
    }
}
