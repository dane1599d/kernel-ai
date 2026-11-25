//! The taskbar and tray (Kai Desk PR 7, ADR 0005 §8): the items repaint
//! whenever what they show changes, a press restores or minimizes, the
//! taskbar wins clicks over windows, overflow becomes "more", the Kai button
//! rings while a card is up, the chip is code's alone, and the clock.

use crate::chat::{Chat, Who};
use crate::chrome;
use crate::desk::{Button, Desk, Effect, Focus, Hit, Input, TrayPart, WindowId, usage};
use crate::geom::{Point, Rect, Size};
use crate::paint::{Bar, Painter, Scene};
use crate::paint_tests::{BAR, FONTS};
use crate::panel::Panel;
use crate::taskbar::{Item, Pill};
use alloc::vec::Vec;
use gfx2d::Image;

const W: i32 = 1280;
const H: i32 = 800;
const SIZES: [(i32, i32); 3] = [(1024, 768), (1280, 800), (1920, 1080)];
const RGB: u32 = (1 << 24) - 1;

fn open(desk: &mut Desk, client: u32, activated: bool) -> WindowId {
    let label = alloc::format!("app{client}");
    desk.open(client, &label, "Win", Size { w: 400, h: 300 }, activated)
        .0
}

/// Whether this frame's damage covers the whole taskbar.
fn bar_damaged(desk: &mut Desk) -> bool {
    let bar = desk.layout().taskbar;
    desk.take_damage()
        .iter()
        .any(|d| d.intersect(&bar) == Some(bar))
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

/// Item `i`'s middle, as the taskbar lays its items out now.
fn item_at(desk: &Desk, i: usize) -> Point {
    let n = desk.taskbar_items().len();
    middle(desk.item_rect(i, n))
}

fn fresh() -> Desk {
    let mut desk = Desk::new(W, H);
    desk.take_damage();
    desk
}

fn paint(desk: &Desk, bar: &Bar, panel: &Panel, chat: &Chat) -> Image {
    paint_in(desk, bar, panel, chat, None, &mut |_, _, _| false)
}

fn paint_in(
    desk: &Desk,
    bar: &Bar,
    panel: &Panel,
    chat: &Chat,
    theme: Option<&look::theme::Theme>,
    content: &mut dyn FnMut(WindowId, &mut gfx2d::Canvas<'_>, Rect) -> bool,
) -> Image {
    let l = desk.layout();
    let (w, h) = (l.width, l.height);
    let mut painter = Painter::new(FONTS).expect("the fonts parse");
    if let Some(theme) = theme {
        painter.set_theme(theme);
    }
    let mut image = Image::new(w, h);
    let mut canvas = image.canvas();
    let scene = Scene {
        desk,
        bar,
        panel,
        chat,
        start: None,
        ask: None,
        preview: None,
        now_ms: 0,
    };
    painter.paint(&mut canvas, Rect::new(0, 0, w, h), &scene, content);
    image
}

fn pixels(image: &Image, r: Rect) -> Vec<u32> {
    (r.y..r.bottom())
        .flat_map(|y| (r.x..r.right()).map(move |x| (x, y)))
        .map(|(x, y)| image.px[(y * image.w + x) as usize] & RGB)
        .collect()
}

#[test]
fn opening_or_closing_a_window_repaints_the_taskbar() {
    let mut desk = fresh();
    // Opened by an app, not a person: no focus change, still a new item.
    let id = open(&mut desk, 1, false);
    assert!(bar_damaged(&mut desk), "a window opened unfocused");
    open(&mut desk, 2, true);
    assert!(bar_damaged(&mut desk), "a window opened with focus");
    desk.close(id);
    assert!(bar_damaged(&mut desk), "a window closed");
}

#[test]
fn a_new_title_focus_order_or_minimize_repaints_the_taskbar() {
    let mut desk = fresh();
    let a = open(&mut desk, 1, true);
    let b = open(&mut desk, 2, true);
    desk.take_damage();
    desk.set_title(a, "Notes");
    assert!(bar_damaged(&mut desk), "a title");
    let mut effects: Vec<Effect> = Vec::new();
    desk.set_focus(Focus::Panel, &mut effects);
    assert!(bar_damaged(&mut desk), "focus to the panel");
    desk.input(Input::Key {
        usage: usage::LEFT_ALT,
        down: true,
    });
    desk.input(Input::Key {
        usage: usage::TAB,
        down: true,
    });
    assert!(bar_damaged(&mut desk), "a raise");
    desk.activate(b, &mut effects);
    desk.take_damage();
    let [.., (_, minimize)] = chrome::controls(desk.window(b).unwrap().frame);
    click(&mut desk, middle(minimize));
    assert!(desk.window(b).unwrap().minimized);
    assert!(bar_damaged(&mut desk), "a minimize");
    desk.activate(b, &mut effects);
    assert!(bar_damaged(&mut desk), "a window back from minimized");
}

#[test]
fn a_commit_alone_leaves_the_taskbar_be() {
    let mut desk = fresh();
    let id = open(&mut desk, 1, true);
    desk.take_damage();
    let content = desk.window(id).unwrap().content();
    let size = Size {
        w: content.w,
        h: content.h,
    };
    desk.commit(id, size, &[Rect::new(0, 0, 10, 10)]);
    assert!(!bar_damaged(&mut desk));
}

/// A press on a minimized app's item brings it back with the keys; a press
/// on the focused app's item minimizes it.
#[test]
fn a_press_restores_and_a_press_on_the_focused_item_minimizes() {
    let mut desk = fresh();
    let a = open(&mut desk, 1, true);
    let _b = open(&mut desk, 2, true);
    desk.minimize(a, &mut Vec::new());
    assert!(desk.window(a).unwrap().minimized);
    // Items: Kai, app1, app2.
    {
        let at = item_at(&desk, 1);
        click(&mut desk, at)
    };
    assert!(
        !desk.window(a).unwrap().minimized,
        "the item did not restore it"
    );
    assert_eq!(desk.focus(), Focus::Window(a));
    {
        let at = item_at(&desk, 1);
        click(&mut desk, at)
    };
    assert!(
        desk.window(a).unwrap().minimized,
        "the focused item did not minimize"
    );
    assert_ne!(desk.focus(), Focus::Window(a));
}

/// One item an app, by its kernel label, never by title: two windows of one
/// app share an item, and an app titled like another keeps its own.
#[test]
fn items_are_matched_by_label_and_never_by_title() {
    let mut desk = fresh();
    let size = Size { w: 300, h: 200 };
    desk.open(1, "notes", "Notes", size, true);
    desk.open(1, "notes", "Notes 2", size, true);
    desk.open(2, "spam", "Notes", size, true);
    let items = desk.taskbar_items();
    let labels: Vec<&str> = items
        .iter()
        .filter_map(|i| match i {
            Item::App(app) => Some(app.label.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(items[0], Item::Kai);
    assert_eq!(labels, ["notes", "spam"]);
    let Item::App(notes) = &items[1] else {
        panic!("notes");
    };
    assert_eq!(notes.windows.len(), 2);
    assert_eq!(notes.pill(&desk), Some(Pill::Running));
}

/// The taskbar is hit before a window dragged under it: a press there
/// reaches the item, never the window.
#[test]
fn the_taskbar_wins_clicks_over_a_window_under_it() {
    let mut desk = fresh();
    let a = open(&mut desk, 1, true);
    let frame = desk.window(a).unwrap().frame;
    desk.input(Input::Motion(Point {
        x: frame.x + 100,
        y: frame.y + 10,
    }));
    desk.input(Input::Button {
        button: Button::Left,
        down: true,
    });
    desk.input(Input::Motion(Point {
        x: frame.x + 100,
        y: H + 500,
    }));
    desk.input(Input::Button {
        button: Button::Left,
        down: false,
    });
    let moved = desk.window(a).unwrap().frame;
    let bar = desk.layout().taskbar;
    assert!(
        moved.y + chrome::TITLE_BAR <= bar.y,
        "a title bar stays above it"
    );
    // A window frame under the taskbar strip: the bar takes the press.
    let under = Point {
        x: moved.x + 50,
        y: bar.y + 20,
    };
    assert!(matches!(desk.hit(under), Hit::Taskbar(_) | Hit::Tray(_)));
    let effects = click(&mut desk, under);
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::Button { window, .. } if *window == a))
    );
}

/// Past the most items a screen allows, the last slot is "more", which opens
/// the Apps sheet; the group never meets the tray.
#[test]
fn overflow_becomes_more_and_the_group_never_meets_the_tray() {
    for (w, h) in SIZES {
        let mut desk = Desk::new(w, h);
        for client in 1..=20 {
            open(&mut desk, client, false);
        }
        let items = desk.taskbar_items();
        let most = deskgeom::taskbar::max_items(w);
        assert_eq!(items.len(), most, "{w}x{h}");
        assert_eq!(items.last(), Some(&Item::More));
        let last = desk.item_rect(most - 1, most);
        assert!(last.right() < desk.layout().tray.area.x, "{w}x{h}");
        let effects = {
            let at = item_at(&desk, most - 1);
            click(&mut desk, at)
        };
        assert!(effects.contains(&Effect::ShowApps), "{w}x{h}");
    }
}

/// The Kai button and the Kai corner open the pane with its input focused;
/// the chip toggles it.
#[test]
fn the_kai_button_opens_start_and_the_corner_and_chip_the_pane() {
    let mut desk = fresh();
    {
        let at = item_at(&desk, 0);
        click(&mut desk, at)
    };
    assert_eq!(desk.focus(), Focus::Panel);
    assert!(desk.start().is_some() && desk.pane().is_none());
    // A press outside Start closes it, and goes nowhere else.
    let effects = click(&mut desk, Point { x: 5, y: 5 });
    assert!(desk.start().is_none());
    assert!(effects.contains(&Effect::Start(false)));
    let chip = desk.layout().tray.chip;
    click(&mut desk, middle(chip));
    assert!(desk.pane().is_some(), "the chip opens it");
    click(&mut desk, middle(chip));
    assert!(desk.pane().is_none(), "the chip closes it");
    let mut desk = fresh();
    // A pointer pushed far past the corner lands on its last pixel.
    let corner = Point {
        x: W + 500,
        y: H + 500,
    };
    click(&mut desk, corner);
    assert_eq!(desk.hit(desk.pointer()), Hit::Tray(TrayPart::Corner));
    assert_eq!(desk.focus(), Focus::Panel);
}

/// Kai's words while the pane is closed badge the chip, until it opens.
#[test]
fn kai_s_words_badge_the_chip_until_the_pane_opens() {
    let mut desk = fresh();
    desk.kai_said();
    assert!(desk.unread());
    desk.set_focus(Focus::Panel, &mut Vec::new());
    assert!(!desk.unread());
    desk.kai_said();
    assert!(!desk.unread(), "said with the pane open: read");
}

/// Security: while a card is up the Kai button wears an `ask` ring, a second
/// trusted cue no app can paint; without one it has none.
#[test]
fn an_ask_card_rings_the_kai_button() {
    let mut desk = fresh();
    let button = desk.item_rect(0, desk.taskbar_items().len());
    let ask = Painter::new(FONTS).unwrap().palette.ask;
    let panel = Panel::new();
    let plain = paint(&desk, &BAR, &panel, &Chat::default());
    assert!(!pixels(&plain, button).contains(&ask));
    desk.show_ask(4, 2);
    let ringed = paint(&desk, &BAR, &panel, &Chat::default());
    let ring = pixels(&ringed, button)
        .iter()
        .filter(|&&p| p == ask)
        .count();
    assert!(ring > 100, "{ring} ask pixels");
}

/// Security: the chip is code's. Whatever the history says, the chip and
/// the Kai button paint as without it.
#[test]
fn a_hostile_said_never_changes_the_chip() {
    let mut chat = Chat::default();
    for text in ["Kai done", "Kai asks to run", "Kai refused", "No model"] {
        chat.push(Who::Kai, text);
    }
    let mut fair = Chat::default();
    fair.push(Who::Kai, "Hello.");
    for (w, h) in SIZES {
        // Each said with the pane closed: the chip's badge shows on both.
        let mut desk = Desk::new(w, h);
        desk.kai_said();
        assert!(desk.unread());
        let panel = Panel::new();
        let with = paint(&desk, &BAR, &panel, &chat);
        let without = paint(&desk, &BAR, &panel, &fair);
        let bar = desk.layout().taskbar;
        assert_eq!(pixels(&with, bar), pixels(&without, bar), "{w}x{h}");
    }
}

/// Seconds at 2026-12-31 23:59:59 UTC.
const NEW_YEARS_EVE: u64 = 1_798_761_599;

/// The worst rollover (midnight on New Year's Eve) changes the clock slot
/// alone, and at most 5 of its 16 px tiles in any row.
#[test]
fn the_clock_rolls_over_inside_its_five_tiles() {
    assert_eq!(crate::clock::date(NEW_YEARS_EVE), "31 Dec UTC");
    rolls_over_inside_its_tiles(0, NEW_YEARS_EVE);
}

/// The same in a fixed offset: the wall's midnight, not UTC's, is the one that
/// rolls the date, and the longer date line ("UTC+5:45") still fits the slot.
#[test]
fn a_fixed_offset_rolls_over_at_its_own_midnight() {
    for zone in [120i16, 345, -570, -720, 840] {
        let midnight = (NEW_YEARS_EVE + 1).saturating_add_signed(-i64::from(zone) * 60);
        assert_eq!(crate::clock::time_in(midnight - 1, zone), "23:59");
        assert_eq!(crate::clock::time_in(midnight, zone), "00:00");
        rolls_over_inside_its_tiles(zone, midnight - 1);
    }
}

/// Painting the second `last` and the one after it, in the zone: only the clock
/// slot differs, in at most 5 tiles of any row.
fn rolls_over_inside_its_tiles(zone: i16, last: u64) {
    for (w, h) in SIZES {
        let desk = Desk::new(w, h);
        let panel = Panel::new();
        let at = |s| Bar {
            utc_s: Some(s),
            zone_min: zone,
            ..BAR
        };
        let before = paint(&desk, &at(last), &panel, &Chat::default());
        let after = paint(&desk, &at(last + 1), &panel, &Chat::default());
        let slot = desk.layout().tray.clock;
        let mut tiles_in_row = [0u32; 2];
        for y in 0..h {
            let row: Vec<i32> = (0..w)
                .filter(|&x| {
                    before.px[(y * w + x) as usize] & RGB != after.px[(y * w + x) as usize] & RGB
                })
                .collect();
            for &x in &row {
                assert!(
                    slot.contains(Point { x, y }),
                    "{w}x{h}: {x},{y} outside the clock"
                );
            }
            let mut tiles: Vec<i32> = row.iter().map(|x| x / 16).collect();
            tiles.dedup();
            assert!(
                tiles.len() <= 5,
                "{w}x{h}: {} tiles in row {y}",
                tiles.len()
            );
            tiles_in_row[usize::from(tiles.is_empty())] += 1;
        }
        assert!(tiles_in_row[0] > 0, "{w}x{h}: nothing rolled over");
    }
}

/// Changing the zone repaints the clock slot and nothing else, and the slot
/// reads the zone's time, not UTC's.
#[test]
fn the_zone_changes_the_clock_slot_alone() {
    use crate::tray::{Shown, damage};
    for (w, h) in SIZES {
        let desk = Desk::new(w, h);
        let panel = Panel::new();
        let at = |zone_min| Bar {
            utc_s: Some(NEW_YEARS_EVE - 7_200),
            zone_min,
            ..BAR
        };
        let utc = paint(&desk, &at(0), &panel, &Chat::default());
        let east = paint(&desk, &at(120), &panel, &Chat::default());
        let slot = desk.layout().tray.clock;
        let differs = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let i = (y * w + x) as usize;
                utc.px[i] & RGB != east.px[i] & RGB
            })
            .collect::<Vec<_>>();
        assert!(!differs.is_empty(), "{w}x{h}: the zone drew nothing");
        assert!(
            differs.iter().all(|&(x, y)| slot.contains(Point { x, y })),
            "{w}x{h}: the zone reached outside the clock"
        );
        // The tray repaints the clock, and only it, when the zone alone changes.
        let mut shown = Shown::default();
        damage(&desk, &panel, &at(0), 0, &mut shown);
        assert_eq!(damage(&desk, &panel, &at(0), 0, &mut shown), []);
        let rects = damage(&desk, &panel, &at(120), 0, &mut shown);
        assert_eq!(rects, [desk.layout().tray.clock], "{w}x{h}");
    }
}

/// With no wall clock there is no clock: the slot is the taskbar's ground.
#[test]
fn no_wall_clock_draws_no_clock() {
    let desk = fresh();
    let image = paint(&desk, &BAR, &Panel::new(), &Chat::default());
    let shell = Painter::new(FONTS).unwrap().palette.shell;
    let slot = desk.layout().tray.clock;
    let inner = Rect::new(slot.x, slot.y + 1, slot.w, slot.h - 1);
    assert!(pixels(&image, inner).iter().all(|&p| p == shell));
}

/// Backlog item 9: a title bar dragged far off stays in reach.
#[test]
fn a_dragged_title_bar_stays_in_reach() {
    let mut desk = fresh();
    let a = open(&mut desk, 1, true);
    let frame = desk.window(a).unwrap().frame;
    desk.input(Input::Motion(Point {
        x: frame.x + 100,
        y: frame.y + 10,
    }));
    desk.input(Input::Button {
        button: Button::Left,
        down: true,
    });
    desk.input(Input::Motion(Point {
        x: -2000,
        y: H + 500,
    }));
    desk.input(Input::Button {
        button: Button::Left,
        down: false,
    });
    let moved = desk.window(a).unwrap().frame;
    let area = desk.work_area();
    assert!(moved.y + chrome::TITLE_BAR <= area.bottom(), "{moved:?}");
    assert!(moved.right() >= area.x + 64, "{moved:?}");
}

/// Each thinking or acting change, and a card, repaints the Kai button; a
/// still desk repaints nothing of the taskbar.
#[test]
fn a_state_or_card_change_repaints_the_kai_button() {
    use crate::panel::State;
    use crate::tray::{Shown, damage};
    let mut desk = fresh();
    open(&mut desk, 1, true);
    let button = desk.item_rect(0, desk.taskbar_items().len());
    let mut panel = Panel::new();
    let mut shown = Shown::default();
    damage(&desk, &panel, &BAR, 0, &mut shown);
    assert!(
        damage(&desk, &panel, &BAR, 0, &mut shown).is_empty(),
        "still"
    );
    for state in [
        State::Thinking { since_ms: 0 },
        State::Done { line: "x".into() },
    ] {
        panel.set_state(state);
        let rects = damage(&desk, &panel, &BAR, 0, &mut shown);
        assert!(rects.contains(&button), "{:?}", panel.state());
    }
    desk.show_ask(3, 1);
    assert!(damage(&desk, &panel, &BAR, 0, &mut shown).contains(&button));
}

/// A new minute repaints the clock alone; the memory repaints only when its
/// 0.1 GiB text changes.
#[test]
fn the_clock_and_memory_repaint_only_on_their_text() {
    use crate::tray::{Shown, damage};
    let desk = Desk::new(1920, 1080);
    let t = desk.layout().tray;
    let panel = Panel::new();
    let at = |s: u64, mib: u64| Bar {
        utc_s: Some(s),
        mem_used_mib: mib,
        ..BAR
    };
    let mut shown = Shown::default();
    damage(&desk, &panel, &at(NEW_YEARS_EVE - 59, 2_150), 0, &mut shown);
    // Same minute, same tenth: nothing.
    assert!(damage(&desk, &panel, &at(NEW_YEARS_EVE, 2_160), 0, &mut shown).is_empty());
    // A new minute: the clock alone.
    let rects = damage(&desk, &panel, &at(NEW_YEARS_EVE + 1, 2_160), 0, &mut shown);
    assert_eq!(rects, [t.clock]);
    // A new tenth of a GiB: the memory alone.
    let rects = damage(&desk, &panel, &at(NEW_YEARS_EVE + 1, 2_300), 0, &mut shown);
    assert_eq!(rects, [t.memory.expect("memory")]);
}

/// While a card is up and Kai acts, the Kai button shows both its ring and
/// its pill.
#[test]
fn the_ring_keeps_the_thinking_pill() {
    use crate::panel::State;
    let mut desk = fresh();
    let mut panel = Panel::new();
    panel.set_state(State::Thinking { since_ms: 0 });
    desk.show_ask(4, 1);
    let button = desk.item_rect(0, desk.taskbar_items().len());
    let image = paint(&desk, &BAR, &panel, &Chat::default());
    let palette = Painter::new(FONTS).unwrap().palette;
    let pill = Rect::new(button.x + 12, button.bottom() - 6, 16, 4);
    assert!(pixels(&image, pill).contains(&palette.ai_mark), "no pill");
    assert!(pixels(&image, button).contains(&palette.ask), "no ring");
}

/// Security invariant 2: the taskbar is painted over the windows. A window
/// whose content is all `ask`, reaching under the taskbar, leaves the
/// taskbar's pixels as they are with the same window drawing nothing.
#[test]
fn the_taskbar_is_painted_over_a_window_under_it() {
    let mut desk = fresh();
    desk.open(1, "spam", "Spam", Size { w: 600, h: 1_200 }, false);
    let ask = Painter::new(FONTS).unwrap().palette.ask;
    let mut content = |_: WindowId, c: &mut gfx2d::Canvas<'_>, r: Rect| {
        c.fill(gfx2d::Rect::new(r.x, r.y, r.w, r.h), (0xFF << 24) | ask);
        true
    };
    let (panel, chat) = (Panel::new(), Chat::default());
    let image = paint_in(&desk, &BAR, &panel, &chat, None, &mut content);
    let bare = paint(&desk, &BAR, &panel, &chat);
    let frame = desk.windows()[0].frame;
    let bar = desk.layout().taskbar;
    assert!(
        frame.bottom() > bar.y,
        "the window reaches under the taskbar"
    );
    assert_eq!(pixels(&image, bar), pixels(&bare, bar));
}

/// High contrast: `hover` is the taskbar's ground, so a hovered item or
/// chip shows a 1 px `focus` frame.
#[test]
fn high_contrast_frames_a_hovered_item_and_the_chip() {
    let theme = look::theme::shipped()
        .into_iter()
        .find(|t| t.name == "high-contrast")
        .expect("shipped");
    let palette = theme.palette();
    assert_eq!(palette.hover, palette.shell);
    let (panel, chat) = (Panel::new(), Chat::default());
    let mut desk = fresh();
    let targets = [desk.item_rect(0, 1), desk.layout().tray.chip];
    for r in targets {
        desk.input(Input::Motion(middle(r)));
        assert!(desk.bar_hovered().is_some());
        let image = paint_in(&desk, &BAR, &panel, &chat, Some(&theme), &mut |_, _, _| {
            false
        });
        let y = r.y + r.h / 2;
        assert_eq!(
            image.px[(y * W + r.x) as usize] & RGB,
            palette.focus,
            "{r:?}"
        );
        assert_ne!(image.px[(y * W + r.x + 1) as usize] & RGB, palette.focus);
    }
}

/// ADR 0005 §8: `dim`, `ai` and `line_strong` are never drawn on `hover`.
/// In every shipped theme: a hovered Kai button, app item and "more", and
/// the hovered chip warming, with no model, or with its badge.
#[test]
fn nothing_on_the_taskbars_hover_is_a_hover_never_token() {
    use look::theme;
    for th in theme::shipped() {
        let p = th.palette();
        let allowed = [
            p.text, p.muted, p.ok, p.accent, p.ai_mark, p.hover, p.shell, p.focus,
        ];
        let never: Vec<u32> = theme::HOVER_NEVER
            .iter()
            .map(|t| p.get(*t))
            .filter(|c| !allowed.contains(c))
            .collect();
        let check = |image: &Image, r: Rect, what: &str| {
            for px in pixels(image, r) {
                assert!(!never.contains(&px), "{} {what}: {px:06x}", th.name);
            }
        };
        // Items: Kai, an app with no glyph (a monogram), and "more".
        let mut desk = Desk::new(1024, 768);
        for client in 1..=12 {
            open(&mut desk, client, false);
        }
        let n = desk.taskbar_items().len();
        let chat = Chat::default();
        for i in [0, 1, n - 1] {
            let r = desk.item_rect(i, n);
            desk.input(Input::Motion(middle(r)));
            let image = paint_in(
                &desk,
                &BAR,
                &Panel::new(),
                &chat,
                Some(&th),
                &mut |_, _, _| false,
            );
            check(&image, r, &alloc::format!("item {i}"));
        }
        // The chip, hovered: warming, no model, and badged.
        let mut desk = Desk::new(1920, 1080);
        let chip = desk.layout().tray.chip;
        desk.input(Input::Motion(middle(chip)));
        desk.kai_said();
        let mut warming = Panel::new();
        warming.set_ready(false);
        let none = Bar {
            model: false,
            ..BAR
        };
        for (bar, panel, what) in [
            (&BAR, &warming, "warming"),
            (&none, &Panel::new(), "no model"),
        ] {
            let image = paint_in(&desk, bar, panel, &chat, Some(&th), &mut |_, _, _| false);
            check(&image, chip, what);
        }
    }
}

/// With no card up, no `ask` pixel anywhere on the taskbar, thinking,
/// acting or warming.
#[test]
fn no_ask_pixel_on_the_taskbar_without_a_card() {
    use crate::panel::State;
    let ask = Painter::new(FONTS).unwrap().palette.ask;
    for (w, h) in SIZES {
        let mut desk = Desk::new(w, h);
        open(&mut desk, 1, true);
        desk.kai_said();
        let bar = desk.layout().taskbar;
        for (state, warm) in [
            (State::Thinking { since_ms: 0 }, true),
            (State::Thinking { since_ms: 0 }, false),
            (
                State::Acting {
                    call: "x()".into(),
                    target: "y".into(),
                },
                true,
            ),
            (State::Idle, false),
        ] {
            let mut panel = Panel::new();
            panel.set_ready(warm);
            panel.set_state(state);
            let image = paint(&desk, &BAR, &panel, &Chat::default());
            assert!(
                !pixels(&image, bar).contains(&ask),
                "{w}x{h} {:?}",
                panel.state()
            );
        }
    }
}

/// At 100 to 200% text and in legible type, the clock, the chip and the
/// memory (a two-digit GiB) fit their slots: nothing is cut, and the
/// clock's two lines do not touch.
#[test]
fn the_clock_and_chip_fit_at_any_text_size() {
    let shipped = look::theme::shipped();
    let dark = &shipped[0];
    let legible = shipped
        .iter()
        .find(|t| t.name == "legible")
        .expect("legible");
    let mut themes = Vec::new();
    for percent in [100, 120, 150, 175, 200] {
        themes.push(dark.sized(false, percent));
    }
    themes.push(legible.sized(true, 150));
    for th in &themes {
        let p = th.palette();
        let desk = Desk::new(1920, 1080);
        let t = desk.layout().tray;
        let mut warming = Panel::new();
        warming.set_ready(false);
        let bar = Bar {
            utc_s: Some(NEW_YEARS_EVE),
            mem_used_mib: 99 * 1024,
            ..BAR
        };
        let image = paint_in(
            &desk,
            &bar,
            &warming,
            &Chat::default(),
            Some(th),
            &mut |_, _, _| false,
        );
        let ink = |r: Rect| -> Vec<(i32, i32)> {
            (r.y..r.bottom())
                .flat_map(|y| (r.x..r.right()).map(move |x| (x, y)))
                .filter(|&(x, y)| image.px[(y * 1920 + x) as usize] & RGB != p.shell)
                .collect()
        };
        let clock = ink(Rect::new(
            t.clock.x,
            t.clock.y + 1,
            t.clock.w,
            t.clock.h - 1,
        ));
        assert!(
            clock.iter().all(|&(x, _)| x > t.clock.x),
            "{}: the clock is cut",
            th.name
        );
        let mut rows: Vec<i32> = clock.iter().map(|&(_, y)| y).collect();
        rows.sort();
        rows.dedup();
        let runs = 1 + rows.windows(2).filter(|w| w[1] > w[0] + 1).count();
        assert_eq!(runs, 2, "{}: the clock's lines touch", th.name);
        let chip = ink(t.chip);
        let right = chip.iter().map(|&(x, _)| x).max().unwrap_or(0);
        assert!(
            right < t.chip.right() - 4,
            "{}: the chip's text is cut",
            th.name
        );
        assert_eq!(crate::tray::memory(&bar), "99.0 GiB");
        let memory = t.memory.expect("memory at 1920");
        let mem = ink(memory);
        let right = mem.iter().map(|&(x, _)| x).max().unwrap_or(0);
        assert!(right < memory.right() - 1, "{}: the memory is cut", th.name);
    }
}
