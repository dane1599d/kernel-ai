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
    let l = desk.layout();
    let (w, h) = (l.width, l.height);
    let mut painter = Painter::new(FONTS).expect("the fonts parse");
    let mut image = Image::new(w, h);
    let mut canvas = image.canvas();
    let scene = Scene {
        desk,
        bar,
        panel,
        chat,
        sheet: None,
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
    assert_eq!(notes.pill(&desk), Pill::Running);
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
fn the_kai_button_corner_and_chip_open_the_pane() {
    let mut desk = fresh();
    {
        let at = item_at(&desk, 0);
        click(&mut desk, at)
    };
    assert_eq!(desk.focus(), Focus::Panel);
    assert!(desk.pane().is_some());
    let chip = desk.layout().tray.chip;
    click(&mut desk, middle(chip));
    assert!(desk.pane().is_none(), "the chip closes it");
    click(&mut desk, middle(chip));
    assert!(desk.pane().is_some(), "and opens it");
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
    for (w, h) in SIZES {
        let desk = Desk::new(w, h);
        let panel = Panel::new();
        let with = paint(&desk, &BAR, &panel, &chat);
        let without = paint(&desk, &BAR, &panel, &Chat::default());
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
    for (w, h) in SIZES {
        let desk = Desk::new(w, h);
        let panel = Panel::new();
        let at = |s| Bar {
            utc_s: Some(s),
            ..BAR
        };
        let before = paint(&desk, &at(NEW_YEARS_EVE), &panel, &Chat::default());
        let after = paint(&desk, &at(NEW_YEARS_EVE + 1), &panel, &Chat::default());
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
