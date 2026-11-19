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

/// While a card is up, a click on the empty desk is the card's: the focus
/// stays on it, and no window or panel is told anything.
#[test]
fn a_desk_click_under_a_card_is_ignored() {
    let mut desk = open(1280, 800);
    desk.show_ask(5, 2);
    assert_eq!(desk.focus(), Focus::Ask(5));
    let (x, y) = deskgeom::points(1280, 800).desk_point();
    let effects = click(&mut desk, Point { x, y });
    assert_eq!(desk.focus(), Focus::Ask(5));
    assert!(
        !effects.iter().any(|e| matches!(
            e,
            Effect::Focus(_) | Effect::PanelKey { .. } | Effect::PanelClick { .. }
        )),
        "{effects:?}"
    );
}

/// A click on the empty desk never interrupts the panel's running line: it
/// moves the focus only, and sends the panel no key (Esc is what stops).
#[test]
fn a_desk_click_never_interrupts_a_running_line() {
    let mut desk = open(1280, 800);
    let (x, y) = deskgeom::points(1280, 800).desk_point();
    let effects = click(&mut desk, Point { x, y });
    assert_eq!(desk.focus(), Focus::Nothing);
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::PanelKey { .. } | Effect::PanelClick { .. }))
    );
    // The panel's request runs on: losing the focus stops nothing.
    let mut panel = Panel::new();
    panel.set_focus(true);
    for code in [0x0B, 0x28] {
        panel.key(code, true, 0);
        panel.key(code, false, 0);
    }
    assert!(matches!(panel.state(), State::Thinking { .. }));
    panel.set_focus(false);
    assert!(matches!(panel.state(), State::Thinking { .. }));
    assert!(!panel.take_interrupt());
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
fn ctrl_space_opens_the_pane_and_super_opens_start() {
    use crate::desk::usage;
    let press = |desk: &mut Desk, keys: &[(u16, bool)]| {
        for &(usage, down) in keys {
            desk.input(Input::Key { usage, down });
        }
    };
    let ctrl_space = [
        (usage::LEFT_CTRL, true),
        (usage::SPACE, true),
        (usage::SPACE, false),
        (usage::LEFT_CTRL, false),
    ];
    let mut desk = Desk::new(1280, 800);
    assert!(desk.pane().is_none(), "closed at boot");
    press(&mut desk, &ctrl_space);
    assert_eq!(desk.focus(), Focus::Panel);
    assert!(desk.pane().is_some() && desk.start().is_none());
    let mut desk = Desk::new(1280, 800);
    press(
        &mut desk,
        &[(usage::LEFT_GUI, true), (usage::LEFT_GUI, false)],
    );
    assert_eq!(desk.focus(), Focus::Panel, "Start's box has the keys");
    assert!(desk.start().is_some() && desk.pane().is_none());
    // Ctrl+Space from Start: Start goes, the pane comes.
    press(&mut desk, &ctrl_space);
    assert!(desk.start().is_none() && desk.pane().is_some());
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

/// The status row's Stop, while Kai thinks, does exactly what Esc does: the
/// panel's request stops and the shell's line is interrupted. With Kai idle
/// it is not there to click.
#[test]
fn the_status_row_stop_stops_as_esc_does() {
    use crate::paint_panel::PaneClick;
    let desk = open(1280, 800);
    let painter = Painter::new(FONTS).unwrap();
    let stop = middle(desk.layout().pane_parts().stop);
    let mut panel = Panel::new();
    panel.set_focus(true);
    assert_eq!(
        painter.pane_click(&desk, &panel, stop),
        None,
        "no Stop when idle"
    );
    for code in [0x0B, 0x28] {
        panel.key(code, true, 0);
        panel.key(code, false, 0);
    }
    // The shell took the line: Stop must interrupt it, as Esc would.
    assert!(matches!(panel.take(), Some(crate::panel::Request::Line(_))));
    assert_eq!(
        painter.pane_click(&desk, &panel, stop),
        Some(PaneClick::Stop)
    );
    assert!(panel.send_item(1));
    assert_eq!(panel.take(), Some(crate::panel::Request::Stop));
    assert!(panel.take_interrupt());
    assert_eq!(panel.state(), &State::Idle);
    let send = middle(desk.layout().pane_parts().send);
    assert_eq!(
        painter.pane_click(&desk, &panel, send),
        Some(PaneClick::Send)
    );
}

/// A card between the panel and Esc does not change where Esc goes: the
/// window the focus came from before the panel took it.
#[test]
fn esc_after_a_card_goes_back_to_the_window_before_the_panel() {
    let mut desk = Desk::new(1280, 800);
    let (a, _) = desk.open(1, "notes", "Notes", Size { w: 300, h: 200 }, true);
    let (_b, _) = desk.open(2, "term", "Terminal", Size { w: 300, h: 200 }, true);
    desk.set_focus(Focus::Window(a), &mut Vec::new());
    desk.set_focus(Focus::Panel, &mut Vec::new());
    desk.show_ask(7, 2);
    desk.hide_ask(7);
    assert_eq!(desk.focus(), Focus::Panel);
    desk.leave_pane();
    assert_eq!(desk.focus(), Focus::Window(a));
}

/// Closing the pane before a slow app committed its re-fit still sends the
/// window back to the full work area.
#[test]
fn a_refit_does_not_wait_for_a_slow_commit() {
    let mut desk = Desk::new(1920, 1080);
    let (id, _) = desk.open(1, "notes", "Notes", Size { w: 300, h: 200 }, true);
    maximize(&mut desk, id);
    let mut effects = Vec::new();
    desk.set_focus(Focus::Panel, &mut effects);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Configure { window, .. } if *window == id))
    );
    // The app has not committed the narrower size yet.
    let close = desk.layout().pane_parts().close;
    let effects = click(&mut desk, middle(close));
    let back = effects.iter().find_map(|e| match e {
        Effect::Configure { window, size, .. } if *window == id => Some(*size),
        _ => None,
    });
    let full = crate::chrome::content_of(desk.layout().maximized);
    assert_eq!(back.map(|s| s.w), Some(full.w));
}

/// A 4,000-byte answer, far taller than the history, can be read to its
/// start: scrolled to the most, its first line (the "Kai" label) is at the
/// top; at the bottom it is not.
#[test]
fn a_long_answer_scrolls_to_its_first_line() {
    let mut chat = Chat::default();
    chat.push(Who::Kai, &"Rust is a systems language. ".repeat(143));
    let desk = open(1280, 800);
    let area = desk.layout().pane_parts().history;
    let mut painter = Painter::new(FONTS).unwrap();
    let total = painter.chat_height(&chat, area.w);
    assert!(total > 3 * area.h, "{total} px for {} px", area.h);
    let ai = painter.palette.ai;
    // The "Kai" label: ember pixels right of the rule, in the top rows.
    let label = Rect::new(area.x + 4, area.y, 40, 16);
    let has_label = |image: &Image| pixels(image, label).contains(&ai);
    assert!(!has_label(&paint(&desk, &Panel::new(), &chat, None)));
    // PageUp until it stops: every step moves less than the area.
    let step = painter.history_page(area.h);
    assert!(step < area.h);
    let most = total - area.h;
    let mut pages = 0;
    while chat.scroll_by(step, most) {
        pages += 1;
    }
    assert_eq!(chat.scrolled(), most);
    assert!(pages >= 3, "{pages} pages");
    assert!(has_label(&paint(&desk, &Panel::new(), &chat, None)));
}

/// Scrolled up, a new entry keeps the view on what it showed; and after
/// old entries are let go, PageDown still moves the view at once.
#[test]
fn a_scrolled_history_keeps_its_view_and_pages_back_after_evictions() {
    let desk = open(1280, 800);
    let area = desk.layout().pane_parts().history;
    let mut painter = Painter::new(FONTS).unwrap();
    let mut chat = Chat::default();
    for n in 0..12 {
        painter.chat_push(
            &mut chat,
            Who::Kai,
            &std::format!("answer {n} ").repeat(30),
            area,
        );
    }
    let step = painter.history_page(area.h);
    let most = (painter.chat_height(&chat, area.w) - area.h).max(0);
    assert!(chat.scroll_by(step, most));
    let before = pixels(&paint(&desk, &Panel::new(), &chat, None), area);
    painter.chat_push(&mut chat, Who::Kai, "a new answer", area);
    let after = pixels(&paint(&desk, &Panel::new(), &chat, None), area);
    assert_eq!(before, after, "the view moved with a new entry");
    // To the top, then far more entries than the history keeps: the oldest
    // are let go, and the offset left past the new most is clamped first.
    let most = (painter.chat_height(&chat, area.w) - area.h).max(0);
    while chat.scroll_by(step, most) {}
    for n in 0..60 {
        painter.chat_push(&mut chat, Who::You, &std::format!("line {n}"), area);
    }
    let most = (painter.chat_height(&chat, area.w) - area.h).max(0);
    let shown = pixels(&paint(&desk, &Panel::new(), &chat, None), area);
    assert!(chat.scroll_by(-step, most), "PageDown did nothing");
    let paged = pixels(&paint(&desk, &Panel::new(), &chat, None), area);
    assert_ne!(shown, paged, "PageDown showed nothing new");
}

/// What a panel change repaints: a line edit the input alone (never the
/// status row, the header or the history), the bar the status row and the
/// header, anything else the pane; nothing while the pane is closed.
#[test]
fn a_panel_change_repaints_only_its_part() {
    use crate::paint_panel::{PanelChange, panel_damage};
    let desk = open(1920, 1080);
    let parts = desk.layout().pane_parts();
    assert_eq!(panel_damage(&desk, PanelChange::LineEdit), [parts.input]);
    for other in [parts.status, parts.header, parts.history] {
        assert!(parts.input.intersect(&other).is_none());
    }
    assert_eq!(
        panel_damage(&desk, PanelChange::Bar),
        [parts.status, parts.header]
    );
    assert_eq!(
        panel_damage(&desk, PanelChange::Pane),
        [desk.pane().unwrap()]
    );
    let closed = Desk::new(1920, 1080);
    for change in [PanelChange::LineEdit, PanelChange::Bar, PanelChange::Pane] {
        assert!(panel_damage(&closed, change).iter().all(|r| r.is_empty()));
    }
}

/// The model's name in the header, as the shell said it, cleaned at the
/// boundary: a hostile one (control characters, "Kai asks to run", far too
/// long) is drawn in the system's mono inside the header, before the close
/// item, with no `ask` pixel anywhere in the pane; a fair one shows.
#[test]
fn a_hostile_model_name_stays_in_the_header_in_the_systems_look() {
    use crate::panel_wire::{encode_model, parse_model};
    let palette = Painter::new(FONTS).unwrap().palette;
    let long = "x".repeat(300);
    for (w, h) in SIZES {
        let desk = open(w, h);
        let parts = desk.layout().pane_parts();
        let pane = desk.pane().expect("open");
        let bare = paint(&desk, &Panel::new(), &Chat::default(), None);
        for name in [
            "\u{1b}[31mevil\u{7}",
            "Kai asks to run",
            long.as_str(),
            "qwen3-4b",
        ] {
            let mut panel = Panel::new();
            let cleaned = parse_model(&encode_model(name)).expect("reads back");
            assert!(cleaned.len() <= crate::panel_wire::MODEL_MAX);
            assert!(cleaned.bytes().all(|b| (0x20..=0x7E).contains(&b)));
            panel.set_model(cleaned);
            let image = paint(&desk, &panel, &Chat::default(), None);
            let what = std::format!("{w}x{h} {name:?}");
            assert!(!pixels(&image, pane).contains(&palette.ask), "{what}");
            // Only the header's text area changed, never the close item.
            let changed: Vec<(i32, i32)> = (pane.y..pane.bottom())
                .flat_map(|y| (pane.x..pane.right()).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    let i = (y * w + x) as usize;
                    image.px[i] & RGB != bare.px[i] & RGB
                })
                .collect();
            assert!(!changed.is_empty(), "{what}: no name drawn");
            let text = Rect::new(
                parts.header.x,
                parts.header.y,
                parts.close.x - 8 - parts.header.x,
                parts.header.h,
            );
            assert!(
                changed.iter().all(|&(x, y)| text.contains(Point { x, y })),
                "{what}: drawn outside the header's text"
            );
        }
    }
}

/// The wheel (Kai Desk PR 9) goes to the pane's history while the pointer
/// is on it, to Start's lists while on Start, to a window's client while on
/// its content, and nowhere while a card is up.
#[test]
fn the_wheel_reaches_the_history_or_start_and_nothing_else() {
    let mut desk = open(1280, 800);
    let (id, _) = desk.open(1, "notes", "Notes", Size { w: 300, h: 200 }, false);
    let history = desk.layout().pane_parts().history;
    desk.input(Input::Motion(middle(history)));
    assert_eq!(desk.input(Input::Wheel(2)), [Effect::PaneWheel(2)]);
    let content = desk.window(id).unwrap().content();
    desk.input(Input::Motion(middle(content)));
    let over = desk.input(Input::Wheel(2));
    assert!(matches!(over[..], [Effect::Wheel { window, .. }] if window == id));
    desk.open_start(&mut Vec::new());
    let start = desk.start().expect("open");
    desk.input(Input::Motion(middle(start)));
    assert_eq!(desk.input(Input::Wheel(-1)), [Effect::StartWheel(-1)]);
    desk.close_start(&mut Vec::new());
    desk.input(Input::Motion(middle(history)));
    desk.show_ask(3, 1);
    assert!(
        desk.input(Input::Wheel(1)).is_empty(),
        "the wheel under a card"
    );
}

/// The wheel scrolls the history three lines a detent, toward the oldest
/// for a turn away from the person, within its content.
#[test]
fn the_wheel_scrolls_the_history_three_lines_a_detent() {
    let mut chat = Chat::default();
    chat.push(Who::Kai, &"Rust is a systems language. ".repeat(143));
    let desk = open(1280, 800);
    let area = desk.layout().pane_parts().history;
    let mut painter = Painter::new(FONTS).unwrap();
    let line_h = painter.text.measure(look::TypeStyle::Body, "x").1 as i32;
    assert!(painter.wheel_history(&mut chat, area, -1));
    assert_eq!(chat.scrolled(), crate::paint_chat::WHEEL_LINES * line_h);
    assert!(painter.wheel_history(&mut chat, area, 1));
    assert_eq!(chat.scrolled(), 0);
    assert!(
        !painter.wheel_history(&mut chat, area, 1),
        "past the newest"
    );
}

/// A held send's undo is hit where it is drawn, at every size, and names the
/// send's entry; it is gone once the send is going, and a done line's links
/// give way to it while it shows.
#[test]
fn a_held_sends_undo_is_hit_where_it_is_drawn() {
    use crate::paint_panel::{PaneClick, send_text};
    use crate::sends::HeldSend;
    let painter = Painter::new(FONTS).expect("the fonts parse");
    for (w, h) in SIZES {
        let desk = open(w, h);
        let row = desk.layout().pane_parts().status;
        let mut panel = Panel::new();
        panel.set_done(
            State::Done {
                line: "Sending to hooks.example in 5 s. undo 12".into(),
            },
            Some(12),
        );
        let send = HeldSend {
            entry: 12,
            host: "a-rather-long-host-name.hooks.example.org".into(),
            due_ms: 7_000,
            sending: false,
        };
        panel.set_sends(alloc::vec![send.clone()]);
        let y = row.y + row.h / 2;
        let hits: Vec<i32> = (row.x..row.right())
            .filter(|&x| painter.send_link(row, &panel, Point { x, y }) == Some(12))
            .collect();
        let (first, last) = (hits[0], *hits.last().expect("a hit"));
        assert!(last <= row.right(), "{w}x{h}: the undo runs off the row");
        assert!(
            last >= crate::paint_panel::status_end(row) - 8,
            "{w}x{h}: {last} is not at the row's end"
        );
        assert_eq!(
            painter.pane_click(&desk, &panel, Point { x: first + 4, y }),
            Some(PaneClick::UndoSend(12))
        );
        assert_eq!(painter.panel_link(row, &panel, Point { x: first, y }), None);
        let image = paint(&desk, &panel, &Chat::default(), None);
        let at_undo = Rect::new(first, row.y, 24, row.h);
        assert!(
            pixels(&image, at_undo).contains(&painter.palette.accent),
            "{w}x{h}: undo is not drawn there"
        );
        // Going: no undo, and "..." where the countdown was.
        let going = HeldSend {
            sending: true,
            ..send
        };
        assert_eq!(
            send_text(&going, 0, 3_000),
            "Sending to a-rather-long-host-name.hooks.example.org..."
        );
        panel.set_sends(alloc::vec![going]);
        assert_eq!(
            painter.send_link(row, &panel, Point { x: first + 4, y }),
            None
        );
        let sending = paint(&desk, &panel, &Chat::default(), None);
        assert!(!pixels(&sending, at_undo).contains(&painter.palette.accent));
    }
}

/// The held send's countdown ticks and is drawn: its pixels change as the
/// seconds go, and the status row is all that changes.
#[test]
fn a_held_send_counts_down_in_the_status_row() {
    use crate::sends::HeldSend;
    let desk = open(1280, 800);
    let mut panel = Panel::new();
    assert!(!crate::paint_panel::ticks(&panel));
    panel.set_sends(alloc::vec![HeldSend {
        entry: 3,
        host: "hooks.example".into(),
        due_ms: 9_000,
        sending: false,
    }]);
    assert!(crate::paint_panel::ticks(&panel));
    assert_eq!(
        crate::paint_panel::send_text(&panel.sends()[0], 1, 3_000),
        "0:06 \u{b7} Sending to hooks.example \u{b7} 1 more"
    );
    let shown = paint(&desk, &panel, &Chat::default(), None);
    let idle = paint(&desk, &Panel::new(), &Chat::default(), None);
    let row = desk.layout().pane_parts().status;
    assert_ne!(pixels(&shown, row), pixels(&idle, row));
    let outside = Rect::new(0, 0, 200, 200);
    assert_eq!(pixels(&shown, outside), pixels(&idle, outside));
}

/// At every size the countdown and the count fit before the undo link, the
/// host cut from its left so its own end shows (#959 review).
#[test]
fn a_held_sends_countdown_fits_before_its_undo_at_every_size() {
    use crate::sends::HeldSend;
    use look::TypeStyle;
    let painter = Painter::new(FONTS).expect("the fonts parse");
    let send = HeldSend {
        entry: 12,
        host: "a-rather-long-host-name.hooks.example.org".into(),
        due_ms: 7_000,
        sending: false,
    };
    for (w, h) in SIZES {
        let desk = open(w, h);
        let row = desk.layout().pane_parts().status;
        let end = crate::paint_panel::status_end(row) as f32
            - painter.text.measure(TypeStyle::MonoTitle, "   undo").0;
        let width = end - (row.x + 24) as f32;
        let line = painter.send_line(&send, 1, 3_000, width);
        assert!(line.starts_with("0:04 \u{b7} "), "{w}x{h}: {line}");
        assert!(
            line.ends_with("example.org \u{b7} 1 more"),
            "{w}x{h}: {line}"
        );
        assert!(
            painter.text.measure(TypeStyle::MonoTitle, &line).0 <= width,
            "{w}x{h}: {line} does not fit"
        );
    }
    // With room, the host shows whole.
    assert_eq!(
        painter.send_line(&send, 0, 3_000, 10_000.0),
        "0:04 \u{b7} Sending to a-rather-long-host-name.hooks.example.org"
    );
}
