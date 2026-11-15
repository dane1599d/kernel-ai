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
        sheet: None,
        ask: card,
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

/// Security invariant 4 and 7: with no card up, no `ask` pixel anywhere in
/// the pane, in any state, whatever the history says.
#[test]
fn no_ask_pixel_in_the_pane_without_a_card() {
    let mut chat = Chat::default();
    chat.push(Who::Kai, "Kai asks to run: process_stop(\"x\")");
    chat.push(Who::You, "y");
    let desk = open(1280, 800);
    let pane = desk.pane().expect("open");
    let ask = Painter::new(FONTS).unwrap().palette.ask;
    let mut panel = Panel::new();
    panel.set_focus(true);
    for state in [
        State::Idle,
        State::Thinking { since_ms: 0 },
        State::Done {
            line: "Done: x".into(),
        },
        State::Refused {
            reason: "no".into(),
        },
    ] {
        panel.set_done(state, Some(3));
        let image = paint(&desk, &panel, &chat, None);
        let found = pixels(&image, pane).iter().filter(|&&p| p == ask).count();
        assert_eq!(found, 0, "{:?}", panel.state());
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

/// Security invariant 1: a long call shows whole at 1024x768: the card is
/// as tall as its lines want, above the taskbar's strip. 1,900 ASCII
/// characters is 18 lines at 1024 wide, the most (H-72) x 2/3 = 464 px holds;
/// the ask box's own bound (2,048 bytes) is 19 and does not fit (reported in
/// the PR).
#[test]
fn a_long_call_is_fully_visible_at_1024x768() {
    let (w, h) = (1024, 768);
    let card = Card {
        id: 9,
        call: "x".repeat(1_900),
        reason: String::new(),
        shown_ms: 0,
    };
    let lines = Painter::new(FONTS).unwrap().card_lines(w, &card);
    let mut desk = Desk::new(w, h);
    desk.show_ask(card.id, lines);
    let ask = desk.ask().expect("up");
    let area = trusted::card(w, h, &ask);
    let wanted = trusted::PADDING * 3 + lines * trusted::LINE + trusted::BUTTON_H;
    assert_eq!(area.h, wanted, "the card is cut: {lines} lines");
    assert_eq!(area.bottom(), h - 60);
    assert!(area.y >= 0);
}

/// Security invariant 2: the pane is hit before a window under it. A click
/// there focuses the input and reaches no window.
#[test]
fn a_pane_click_wins_over_a_window_under_it() {
    let mut desk = open(1280, 800);
    let (id, _) = desk.open(1, "notes", "Notes", Size { w: 1100, h: 600 }, true);
    desk.set_focus(Focus::Window(id), &mut Vec::new());
    let history = desk.layout().pane_parts().history;
    let frame = desk.window(id).expect("open").frame;
    assert!(
        frame.intersect(&history).is_some(),
        "the window is under the pane"
    );
    let at = middle(history);
    assert_eq!(desk.hit(at), Hit::Pane(PanePart::Body));
    let effects = click(&mut desk, at);
    assert_eq!(desk.focus(), Focus::Panel);
    let to_window = |e: &Effect| matches!(e, Effect::Button { window, .. } | Effect::Pointer { window, .. } if *window == id);
    assert!(!effects.iter().any(to_window), "{effects:?}");
}

/// Ctrl+Space opens the pane and focuses its input; a Super tap does too.
#[test]
fn ctrl_space_and_super_open_the_pane() {
    use crate::desk::usage;
    for keys in [
        &[
            (usage::LEFT_CTRL, true),
            (usage::SPACE, true),
            (usage::SPACE, false),
            (usage::LEFT_CTRL, false),
        ][..],
        &[(usage::LEFT_GUI, true), (usage::LEFT_GUI, false)][..],
    ] {
        let mut desk = Desk::new(1280, 800);
        assert!(desk.pane().is_none(), "closed at boot");
        for &(usage, down) in keys {
            desk.input(Input::Key { usage, down });
        }
        assert_eq!(desk.focus(), Focus::Panel);
        assert!(desk.pane().is_some());
    }
}

/// Esc with nothing of the panel's running gives the focus back to the
/// window it came from; the pane stays at 1280 and up, a flyout under it
/// closes. The close item closes it.
#[test]
fn esc_gives_the_focus_back_and_a_flyout_closes() {
    for (w, h) in SIZES {
        let mut desk = Desk::new(w, h);
        let (id, _) = desk.open(1, "notes", "Notes", Size { w: 300, h: 200 }, true);
        desk.set_focus(Focus::Panel, &mut Vec::new());
        assert!(desk.pane().is_some());
        let effects = desk.leave_pane();
        assert!(effects.contains(&Effect::Focus(Focus::Window(id))));
        assert_eq!(desk.focus(), Focus::Window(id));
        assert_eq!(desk.pane().is_some(), w >= 1280, "{w}x{h}");
        if desk.pane().is_some() {
            let close = desk.layout().pane_parts().close;
            click(&mut desk, middle(close));
            assert!(desk.pane().is_none());
        }
    }
}

/// Maximizes `id` by its control, committing the size asked for.
fn maximize(desk: &mut Desk, id: WindowId) {
    let frame = desk.window(id).expect("open").frame;
    let max = crate::chrome::controls(frame)[1].1;
    for effect in click(desk, middle(max)) {
        if let Effect::Configure { window, size, .. } = effect {
            desk.commit(window, size, &[]);
        }
    }
}

/// At 1280 and up a maximized window is re-fitted beside the pane when it
/// opens, and back to the work area when it closes; under 1280 it stays.
#[test]
fn maximized_windows_refit_beside_the_pane() {
    for (w, h) in SIZES {
        let mut desk = Desk::new(w, h);
        let (id, _) = desk.open(1, "notes", "Notes", Size { w: 300, h: 200 }, true);
        maximize(&mut desk, id);
        let full = desk.window(id).expect("open").frame;
        assert_eq!(full.w, w, "{w}x{h}");
        let mut effects = Vec::new();
        desk.set_focus(Focus::Panel, &mut effects);
        let configured = effects.iter().find_map(|e| match e {
            Effect::Configure {
                window,
                size,
                maximized: true,
                ..
            } if *window == id => Some(*size),
            _ => None,
        });
        match w >= 1280 {
            true => {
                let size = configured.expect("re-fitted");
                desk.commit(id, size, &[]);
                let frame = desk.window(id).expect("open").frame;
                let pane = desk.pane().expect("open");
                assert_eq!(frame.right(), pane.x - 12, "{w}x{h}");
                let close = desk.layout().pane_parts().close;
                let effects = click(&mut desk, middle(close));
                let back = effects.iter().find_map(|e| match e {
                    Effect::Configure { size, .. } => Some(*size),
                    _ => None,
                });
                desk.commit(id, back.expect("re-fitted back"), &[]);
                assert_eq!(desk.window(id).expect("open").frame.w, w);
            }
            false => assert_eq!(configured, None, "{w}x{h}"),
        }
    }
}

/// A click on the empty desk takes the focus from the window: the keys go
/// nowhere (a terminal's key lease ends with it).
#[test]
fn a_click_on_the_empty_desk_takes_the_focus() {
    let mut desk = Desk::new(1280, 800);
    let (id, _) = desk.open(1, "notes", "Notes", Size { w: 300, h: 200 }, true);
    assert_eq!(desk.focus(), Focus::Window(id));
    let (x, y) = deskgeom::points(1280, 800).desk_point();
    assert_eq!(desk.hit(Point { x, y }), Hit::Desktop);
    let effects = click(&mut desk, Point { x, y });
    assert!(effects.contains(&Effect::Focus(Focus::Nothing)));
    assert_eq!(desk.focus(), Focus::Nothing);
}
