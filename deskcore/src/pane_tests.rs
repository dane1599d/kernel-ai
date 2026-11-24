//! The Kai pane (Kai Desk PR 6, ADR 0005 §7-§8): its states, input and
//! history where they belong, and the trusted layer's invariants with it
//! open. Each security test is one the plan requires.

use crate::askcard::Card;
use crate::chat::{Chat, Who};
use crate::desk::{Button, Desk, Effect, Focus, Hit, Input, PanePart, WindowId};
use crate::geom::{Point, Rect, Size};
use crate::paint::{Painter, Scene};
use crate::paint_tests::{BAR, FONTS};
use crate::panel::{Panel, State};
use crate::trusted;
use alloc::string::String;
use alloc::vec::Vec;
use gfx2d::Image;

const SIZES: [(i32, i32); 3] = [(1024, 768), (1280, 800), (1920, 1080)];
const RGB: u32 = (1 << 24) - 1;

/// A desk with the pane open, its input focused.
fn open(w: i32, h: i32) -> Desk {
    let mut desk = Desk::new(w, h);
    desk.set_focus(Focus::Panel, &mut Vec::new());
    desk
}

fn paint(desk: &Desk, panel: &Panel, chat: &Chat, card: Option<&Card>) -> Image {
    let (w, h) = desk_size(desk);
    let mut painter = Painter::new(FONTS).expect("the fonts parse");
    let mut image = Image::new(w, h);
    let mut canvas = image.canvas();
    let scene = Scene {
        desk,
        bar: &BAR,
        panel,
        chat,
        start: None,
        ask: card,
        preview: None,
        now_ms: 3_000,
    };
    let all = Rect::new(0, 0, w, h);
    painter.paint(&mut canvas, all, &scene, &mut |_, _, _| false);
    if let Ok(dir) = std::env::var("DESK_PREVIEW") {
        let mut ppm = std::format!("P6\n{w} {h}\n255\n").into_bytes();
        for p in &image.px {
            ppm.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
        }
        let name = std::format!("{dir}/pane-{w}x{h}-{}.ppm", image.px.len() % 997);
        std::fs::write(name, ppm).expect("the preview is written");
    }
    image
}

fn desk_size(desk: &Desk) -> (i32, i32) {
    let l = desk.layout();
    (l.width, l.height)
}

fn pixels(image: &Image, r: Rect) -> Vec<u32> {
    (r.y..r.bottom())
        .flat_map(|y| (r.x..r.right()).map(move |x| (x, y)))
        .map(|(x, y)| image.px[(y * image.w + x) as usize] & RGB)
        .collect()
}

fn middle(r: Rect) -> Point {
    Point {
        x: r.x + r.w / 2,
        y: r.y + r.h / 2,
    }
}

fn click(desk: &mut Desk, at: Point) -> Vec<Effect> {
    let mut effects = desk.input(Input::Motion(at));
    for down in [true, false] {
        effects.extend(desk.input(Input::Button {
            button: Button::Left,
            down,
        }));
    }
    effects
}

/// Every state the status row shows, and the input listening: previews
/// with `DESK_PREVIEW=dir`.
#[test]
fn the_pane_paints_each_state() {
    let desk = open(1280, 800);
    let chat = Chat::default();
    let mut panel = Panel::new();
    let idle = paint(&desk, &panel, &chat, None);
    panel.set_focus(true);
    for code in [0x0B, 0x0C] {
        panel.key(code, true, 0);
        panel.key(code, false, 0);
    }
    let listening = paint(&desk, &panel, &chat, None);
    assert_ne!(idle.px, listening.px);
    let mut seen = alloc::vec![listening.px];
    for state in [
        State::Thinking { since_ms: 0 },
        State::Acting {
            call: "notes_add(\"milk\")".into(),
            target: "Notes".into(),
        },
        State::Done {
            line: "Done: policy round-robin (was interactive).".into(),
        },
        State::Refused {
            reason: "Refused: notes is protected".into(),
        },
    ] {
        panel.set_state(state);
        let image = paint(&desk, &panel, &chat, None);
        assert!(!seen.contains(&image.px), "two states paint alike");
        seen.push(image.px);
    }
}

/// Typed past the input's width, the line scrolls: the ember caret is in
/// the input, before the send item (the founder's VirtualBox test).
#[test]
fn a_long_line_keeps_the_caret_in_the_input() {
    let desk = open(1280, 800);
    let mut panel = Panel::new();
    panel.set_focus(true);
    for _ in 0..5 {
        for code in 0x04..=0x1D {
            panel.key(code, true, 0);
            panel.key(code, false, 0);
        }
    }
    assert_eq!(panel.cursor(), 130);
    let image = paint(&desk, &panel, &Chat::default(), None);
    let parts = desk.layout().pane_parts();
    let ember = Painter::new(FONTS).unwrap().palette.ai;
    let tail = Rect::new(parts.send.x - 80, parts.input.y + 3, 80, parts.input.h - 6);
    let caret = pixels(&image, tail).iter().filter(|&&p| p == ember).count();
    assert!(caret > 20, "the caret is not in the input: {caret}");
}

/// While the model warms the pane says "Kai is getting ready", in the input
/// and the status row; once warm it paints as it did before.
#[test]
fn the_pane_says_kai_is_getting_ready_until_the_model_is_warm() {
    let desk = open(1280, 800);
    let chat = Chat::default();
    let mut panel = Panel::new();
    let idle = paint(&desk, &panel, &chat, None);
    assert!(panel.set_ready(false));
    let warming = paint(&desk, &panel, &chat, None);
    assert_ne!(idle.px, warming.px);
    panel.set_state(State::Thinking { since_ms: 0 });
    let waiting = paint(&desk, &panel, &chat, None);
    assert!(panel.set_ready(true));
    let thinking = paint(&desk, &panel, &chat, None);
    assert_ne!(waiting.px, thinking.px);
    panel.set_state(State::Idle);
    assert_eq!(paint(&desk, &panel, &chat, None).px, idle.px);
    assert!(!panel.set_ready(true), "no change, no repaint");
}

/// A done line's undo and show are hit where they are drawn, undo first,
/// and only in the status row; none without the entry they act on.
#[test]
fn undo_and_show_are_hit_where_they_are_drawn() {
    let desk = open(1280, 800);
    let painter = Painter::new(FONTS).expect("the fonts parse");
    let row = desk.layout().pane_parts().status;
    let line = "Done: policy round-robin (was interactive).";
    let mut done = Panel::new();
    done.set_done(State::Done { line: line.into() }, Some(7));
    let y = row.y + row.h / 2;
    let found: Vec<(i32, &str)> = (row.x..row.right())
        .filter_map(|x| {
            painter
                .panel_link(row, &done, Point { x, y })
                .map(|w| (x, w))
        })
        .collect();
    let first = |word: &str| found.iter().find(|(_, w)| *w == word).map(|(x, _)| *x);
    let (undo, show) = (first("undo").expect("undo"), first("show").expect("show"));
    assert!(undo < show, "{undo} {show}");
    // Pinned to the row's right end, so a long line never pushes them off.
    let last = found.iter().map(|(x, _)| *x).max().expect("links");
    assert!(
        last >= crate::paint_panel::status_end(row) - 8,
        "{last} is not at the row's end"
    );
    assert_eq!(
        painter.panel_link(row, &Panel::new(), Point { x: show, y }),
        None
    );
    // Drawn: the accent links are where the hits are.
    let image = paint(&desk, &done, &Chat::default(), None);
    let accent = painter.palette.accent;
    let at_undo = Rect::new(undo, row.y, 24, row.h);
    assert!(
        pixels(&image, at_undo).contains(&accent),
        "undo is not drawn there"
    );
    // Off the row: nothing.
    assert_eq!(
        painter.panel_link(
            row,
            &done,
            Point {
                x: undo,
                y: row.y - 5
            }
        ),
        None
    );
    let mut bare = Panel::new();
    bare.set_done(State::Done { line: line.into() }, None);
    assert_eq!(painter.panel_link(row, &bare, Point { x: show, y }), None);
}

/// A few lines of history, Kai's words in its own look, at every size: drawn
/// in the history rectangle alone, and the status row and input unchanged.
#[test]
fn the_history_draws_only_in_its_rectangle() {
    let mut chat = Chat::default();
    chat.push(Who::You, "what is rust?");
    chat.push(
        Who::Kai,
        &"Rust is a systems programming language. ".repeat(40),
    );
    chat.push(Who::Note, "it asks first: ask in the terminal");
    for (w, h) in SIZES {
        let mut desk = open(w, h);
        desk.open(1, "notes", "Notes", Size { w: 700, h: 400 }, false);
        let panel = Panel::new();
        let with = paint(&desk, &panel, &chat, None);
        let without = paint(&desk, &panel, &Chat::default(), None);
        let area = desk.layout().pane_parts().history;
        let changed: Vec<(i32, i32)> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let i = (y * w + x) as usize;
                with.px[i] & RGB != without.px[i] & RGB
            })
            .collect();
        assert!(changed.len() > 1_000, "{w}x{h}: {}", changed.len());
        assert!(
            changed.iter().all(|&(x, y)| area.contains(Point { x, y })),
            "{w}x{h}: the history drew outside its rectangle"
        );
        // Kai's ember rule runs down its words' left edge.
        let ember = Painter::new(FONTS).unwrap().palette.ai;
        let rule = (area.y..area.bottom())
            .filter(|&y| with.px[(y * w + area.x) as usize] & RGB == ember)
            .count();
        assert!(rule > 30, "{w}x{h}: the AI's rule is {rule} px tall");
    }
}

/// Security invariant 3: whatever the history says ("Kai done", "Kai asks
/// to run"), the status row, the header and the input paint as without it:
/// the state line reads only code's state.
#[test]
fn a_hostile_said_never_changes_the_status_pixels() {
    let mut chat = Chat::default();
    for text in [
        "Kai done",
        "Kai asks to run",
        "Done: everything (was nothing)",
        "Refused",
    ] {
        chat.push(Who::Kai, text);
    }
    for (w, h) in SIZES {
        let desk = open(w, h);
        let parts = desk.layout().pane_parts();
        let mut panel = Panel::new();
        for state in [State::Idle, State::Thinking { since_ms: 0 }] {
            panel.set_state(state);
            let with = paint(&desk, &panel, &chat, None);
            let without = paint(&desk, &panel, &Chat::default(), None);
            for part in [parts.status, parts.header, parts.input] {
                assert_eq!(
                    pixels(&with, part),
                    pixels(&without, part),
                    "{w}x{h} {part:?}"
                );
            }
        }
    }
}

/// Security invariants 4 and 7: with no card up, no `ask` pixel anywhere in
/// the pane, and nothing in the card's look (its `raised` ground), in any
/// state, warming or not, focused or showing the placeholder, at every size,
/// whatever the history says.
#[test]
fn no_ask_pixel_in_the_pane_without_a_card() {
    let mut chat = Chat::default();
    chat.push(Who::Kai, "Kai asks to run: process_stop(\"x\")");
    chat.push(Who::You, "y");
    let palette = Painter::new(FONTS).unwrap().palette;
    let states = [
        State::Idle,
        State::Thinking { since_ms: 0 },
        State::Acting {
            call: "notes_add(\"milk\")".into(),
            target: "Notes".into(),
        },
        State::Done {
            line: "Done: x".into(),
        },
        State::Refused {
            reason: "no".into(),
        },
    ];
    for (w, h) in SIZES {
        let desk = open(w, h);
        let pane = desk.pane().expect("open");
        for (warming, focused) in [(false, true), (true, true), (false, false), (true, false)] {
            for state in &states {
                let mut panel = Panel::new();
                panel.set_focus(focused);
                panel.set_ready(!warming);
                panel.set_done(state.clone(), Some(3));
                let image = paint(&desk, &panel, &chat, None);
                let shown = pixels(&image, pane);
                let what = std::format!("{w}x{h} warming={warming} focused={focused} {state:?}");
                assert!(!shown.contains(&palette.ask), "ask in the pane: {what}");
                assert!(
                    !shown.contains(&palette.raised),
                    "the card's ground: {what}"
                );
            }
        }
    }
}

/// Security invariant 1: the card paints the same pixels with the pane open
/// under it as with it closed, at every size.
#[test]
fn the_card_paints_the_same_with_the_pane_open() {
    let card = Card {
        id: 41,
        call: "process_stop(\"build-3\")".into(),
        reason: "build-3 has used a whole CPU for ten minutes.".into(),
        shown_ms: 0,
    };
    for (w, h) in SIZES {
        let lines = Painter::new(FONTS).unwrap().card_lines(w, &card);
        let mut closed = Desk::new(w, h);
        closed.show_ask(card.id, lines);
        let mut opened = open(w, h);
        opened.show_ask(card.id, lines);
        // Under 1280 the pane is a flyout: the card took the focus, so it
        // closed; at 1280 and up it stays open under the card.
        assert_eq!(opened.pane().is_some(), w >= 1280);
        assert!(closed.pane().is_none());
        let panel = Panel::new();
        let a = paint(&closed, &panel, &Chat::default(), Some(&card));
        let b = paint(&opened, &panel, &Chat::default(), Some(&card));
        let area = trusted::card(w, h, &opened.ask().expect("up"));
        assert_eq!(pixels(&a, area), pixels(&b, area), "{w}x{h}");
    }
}

/// Security invariant 1: a call that fills the whole 2,048-byte ask box
/// shows whole at 1024x768: the card is as tall as its lines want, every
/// line is drawn (the last one has ink), and the buttons are on the screen.
#[test]
fn a_call_filling_the_ask_box_is_fully_visible_at_1024x768() {
    let (w, h) = (1024, 768);
    // The id line, the call and its newline: the box's 2,048 bytes, with
    // no reason.
    let card = Card {
        id: 9,
        call: "x".repeat(2_048 - 3),
        reason: String::new(),
        shown_ms: 0,
    };
    let painter = Painter::new(FONTS).unwrap();
    let lines = painter.card_lines(w, &card);
    assert!(trusted::fits(w, h, lines), "{lines} lines");
    let mut desk = Desk::new(w, h);
    desk.show_ask(card.id, lines);
    let ask = desk.ask().expect("up");
    let area = trusted::card(w, h, &ask);
    assert_eq!(
        area.h,
        trusted::wanted(lines),
        "the card is cut: {lines} lines"
    );
    assert_eq!(area.bottom(), h - 60);
    let screen = Rect::new(0, 0, w, h);
    let (yes, no) = trusted::buttons(area);
    for r in [area, yes, no] {
        assert_eq!(r.intersect(&screen), Some(r), "{r:?} off the screen");
    }
    // The last call line, below the title: drawn, not cut.
    let image = paint(&desk, &Panel::new(), &Chat::default(), Some(&card));
    let last = Rect::new(
        area.x + trusted::PADDING,
        area.y + trusted::PADDING + (lines - 1) * trusted::LINE,
        200,
        trusted::LINE,
    );
    let raised = painter.palette.raised;
    let ink = pixels(&image, last)
        .iter()
        .filter(|&&p| p != raised)
        .count();
    assert!(ink > 50, "the last line is not drawn ({ink})");
}

/// A call too long to show whole even at the work height does not fit, so
/// `desktop` refuses its ask rather than draw part of it.
#[test]
fn a_call_too_long_for_the_work_height_does_not_fit() {
    for (w, h) in SIZES {
        let max = desk_layout(w, h).card_max_h;
        let most = (max - trusted::wanted(1)) / trusted::LINE + 1;
        assert!(trusted::fits(w, h, most), "{w}x{h}");
        assert!(!trusted::fits(w, h, most + 1), "{w}x{h}");
    }
}

fn desk_layout(w: i32, h: i32) -> deskgeom::Layout {
    Desk::new(w, h).layout()
}

#[path = "pane_input_tests.rs"]
mod input;
