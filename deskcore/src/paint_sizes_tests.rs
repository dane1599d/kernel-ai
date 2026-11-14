//! The panel, its caret and the chat history at the sizes the founder's
//! machines run (backlog test 4): VirtualBox's 1024x768, the 1280x800 the
//! goldens use, and 1920x1080. Each is drawn only where it belongs: the bar
//! inside the panel's rectangle, the caret inside its input, the history
//! between the top bar and the panel.

use crate::chat::{Chat, Who};
use crate::desk::Desk;
use crate::geom::Rect;
use crate::paint::{Painter, Scene};
use crate::paint_chat::chat_area;
use crate::paint_tests::{BAR, FONTS};
use crate::panel::{Panel, State};
use alloc::vec::Vec;
use gfx2d::Image;

const SIZES: [(i32, i32); 3] = [(1024, 768), (1280, 800), (1920, 1080)];

fn paint(desk: &Desk, panel: &Panel, chat: &Chat, w: i32, h: i32) -> Image {
    let mut painter = Painter::new(FONTS).expect("the fonts parse");
    let mut image = Image::new(w, h);
    let mut canvas = image.canvas();
    let scene = Scene {
        desk,
        bar: &BAR,
        panel,
        chat,
        ask: None,
        now_ms: 0,
    };
    painter.paint(
        &mut canvas,
        Rect::new(0, 0, w, h),
        &scene,
        &mut |_, _, _| false,
    );
    image
}

/// A pixel's colour, its alpha left out.
fn rgb(px: u32) -> u32 {
    px & ((1 << 24) - 1)
}

/// The points where `a` and `b` differ.
fn changed(a: &Image, b: &Image, w: i32, h: i32) -> Vec<(i32, i32)> {
    (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .filter(|&(x, y)| rgb(a.px[(y * w + x) as usize]) != rgb(b.px[(y * w + x) as usize]))
        .collect()
}

fn inside(r: Rect, (x, y): (i32, i32)) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

/// A line typed far past the input's width, and a long result on the right:
/// both stay inside the panel, and the caret inside its input, at each size.
#[test]
fn the_panel_and_its_caret_stay_inside_the_panel_at_each_size() {
    for (w, h) in SIZES {
        let desk = Desk::new(w, h);
        let idle = Panel::new();
        let mut panel = Panel::new();
        panel.set_focus(true);
        for _ in 0..8 {
            for code in 0x04..=0x1D {
                panel.key(code, true, 0);
                panel.key(code, false, 0);
            }
        }
        panel.set_state(State::Done {
            line: "Done: policy round-robin (was interactive); notes saved; ".repeat(4),
        });
        let chat = Chat::default();
        let before = paint(&desk, &idle, &chat, w, h);
        let after = paint(&desk, &panel, &chat, w, h);
        let area = desk.panel();
        let moved = changed(&before, &after, w, h);
        assert!(!moved.is_empty(), "{w}x{h}: nothing drew");
        assert!(
            moved.iter().all(|&p| inside(area, p)),
            "{w}x{h}: the panel drew outside its rectangle"
        );
        // The ember caret is in the input's right part, the left half.
        let ember = rgb(Painter::new(FONTS).unwrap().palette.ai);
        let (x0, x1) = (area.x + area.w / 4, area.x + area.w / 2);
        let caret = (area.y + 3..area.bottom())
            .flat_map(|y| (x0..x1).map(move |x| (x, y)))
            .filter(|&(x, y)| rgb(after.px[(y * w + x) as usize]) == ember)
            .count();
        assert!(
            caret > 20,
            "{w}x{h}: the caret is not in the input ({caret})"
        );
    }
}

/// The history, a long answer included, draws only in its area, which sits
/// between the top bar and the panel; the bar is the same with or without it.
#[test]
fn the_history_stays_in_its_area_at_each_size() {
    let mut chat = Chat::default();
    chat.push(Who::You, "what is rust?");
    chat.push(
        Who::Kai,
        &"Rust is a systems programming language. ".repeat(30),
    );
    chat.push(Who::Kai, &"x".repeat(1_500));
    chat.push(Who::Note, "Kai answered above.");
    for (w, h) in SIZES {
        let desk = Desk::new(w, h);
        let panel = Panel::new();
        let with = paint(&desk, &panel, &chat, w, h);
        let without = paint(&desk, &panel, &Chat::default(), w, h);
        let area = chat_area(&desk);
        assert!(area.y >= desk.top_bar().bottom() && area.bottom() <= desk.panel().y);
        assert!(area.x >= 0 && area.right() <= w, "{w}x{h}: {area:?}");
        let moved = changed(&with, &without, w, h);
        assert!(moved.len() > 1_000, "{w}x{h}: {} pixels", moved.len());
        assert!(
            moved.iter().all(|&p| inside(area, p)),
            "{w}x{h}: the history drew outside its area"
        );
    }
}
