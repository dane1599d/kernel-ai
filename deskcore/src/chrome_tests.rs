//! Kai Desk's window chrome (PR 5): the title row's label and title, the
//! controls' hover, and square corners when maximized.

use crate::chrome::{self, Part};
use crate::desk::{Button, Desk, Input, WindowId};
use crate::geom::{Point, Rect, Size};
use crate::paint::{Painter, Scene};
use crate::paint_tests::{BAR, FONTS};
use crate::panel::Panel;
use alloc::vec::Vec;
use gfx2d::Image;

const W: i32 = 1280;
const H: i32 = 800;
/// The colour bits of a pixel, its alpha byte dropped.
const RGB: u32 = (1 << 24) - 1;

/// The desk painted whole.
fn painted(desk: &Desk) -> Image {
    let mut painter = Painter::new(FONTS).expect("the fonts parse");
    let mut image = Image::new(W, H);
    let mut canvas = image.canvas();
    let scene = Scene {
        desk,
        bar: &BAR,
        panel: &Panel::new(),
        chat: &crate::chat::Chat::default(),
        sheet: None,
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

/// The pixels of `r` in `image`, row by row.
fn pixels(image: &Image, r: Rect) -> Vec<u32> {
    (r.y..r.bottom())
        .flat_map(|y| (r.x..r.right()).map(move |x| (x, y)))
        .map(|(x, y)| image.px[(y * W + x) as usize] & RGB)
        .collect()
}

/// One window of `label` titled `title`, focused.
fn one(label: &str, title: &str) -> (Desk, WindowId) {
    let mut desk = Desk::new(W, H);
    let (id, _) = desk.open(1, label, title, Size { w: 400, h: 300 }, true);
    (desk, id)
}

/// The title bar's text, from the icon to the first control.
fn text_area(desk: &Desk, id: WindowId) -> Rect {
    let frame = desk.window(id).expect("open").frame;
    let bar = chrome::title_bar(frame);
    let end = chrome::controls(frame)[2].1.x;
    Rect::new(bar.x, bar.y, end - bar.x, bar.h)
}

/// ADR 0005 §7: an app cannot pose as another by its title. The kernel's
/// label is drawn before it, so the same title under another label paints
/// another bar.
#[test]
fn an_app_titled_like_another_still_shows_its_label() {
    let (notes, a) = one("notes", "Notes");
    let (spam, b) = one("spam", "Notes");
    assert_eq!(text_area(&notes, a), text_area(&spam, b));
    let area = text_area(&notes, a);
    assert_ne!(
        pixels(&painted(&notes), area),
        pixels(&painted(&spam), area),
        "the label is not drawn"
    );
}

/// A long title is cut at its end, before the controls: the label, and the
/// controls, paint as they do under a short one.
#[test]
fn the_label_then_the_title_is_cut_from_the_titles_end() {
    let (short, a) = one("notes", "N");
    let (long, b) = one("notes", &"Groceries and more ".repeat(20));
    let (short, long) = (painted(&short), painted(&long));
    let area = text_area(&one("notes", "N").0, a);
    assert_eq!(area, text_area(&one("notes", "x").0, b));
    // The icon and the label "notes": the bar's first 70 px.
    let head = Rect::new(area.x, area.y, 70, area.h);
    assert_eq!(pixels(&short, head), pixels(&long, head));
    // Past the text's end, the controls are untouched by the title.
    let controls = Rect::new(area.right(), area.y, 3 * chrome::CONTROL_W, area.h);
    assert_eq!(pixels(&short, controls), pixels(&long, controls));
    assert_ne!(pixels(&short, area), pixels(&long, area));
}

/// The pointer over a control hovers it, and damages it; moving off clears
/// the hover and damages it again. Close hovers in `error`.
#[test]
fn a_hovered_control_is_tracked_damaged_and_painted() {
    let (mut desk, id) = one("notes", "Notes");
    let frame = desk.window(id).expect("open").frame;
    let [(_, close), (_, max), _] = chrome::controls(frame);
    let middle = |r: Rect| Point {
        x: r.x + r.w / 2,
        y: r.y + r.h / 2,
    };
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

/// A maximized window has square corners; one that is not, rounded ones.
#[test]
fn a_maximized_window_is_square() {
    let (mut desk, id) = one("notes", "Notes");
    let corner = |desk: &Desk| {
        let f = desk.window(id).expect("open").frame;
        let image = painted(desk);
        pixels(&image, Rect::new(f.x, f.y, 1, 1))[0]
    };
    let line = look::DARK.line_strong;
    assert_ne!(corner(&desk), line, "a window's corner is rounded");
    let max = chrome::controls(desk.window(id).expect("open").frame)[1].1;
    desk.input(Input::Motion(Point {
        x: max.x + max.w / 2,
        y: max.y + max.h / 2,
    }));
    for down in [true, false] {
        desk.input(Input::Button {
            button: Button::Left,
            down,
        });
    }
    assert!(desk.window(id).expect("open").maximized());
    assert_eq!(corner(&desk), line, "a maximized window's corner is square");
}
