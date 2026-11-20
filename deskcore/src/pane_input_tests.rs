//! The Kai pane's input: clicks and keys beside cards and windows, focus,
//! the history's scrolling, and the held sends' row.

use super::*;

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
