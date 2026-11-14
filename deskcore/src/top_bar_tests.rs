//! The top bar's tabs repaint whenever what they show changes: the windows,
//! their stacking order, titles, focus and minimized state. Before this, only
//! a full repaint (a theme change) drew a window opened after the first frame.

use crate::chrome;
use crate::desk::{Button, Desk, Effect, Focus, Input, WindowId, usage};
use crate::geom::{Point, Rect, Size};
use alloc::vec::Vec;

const W: i32 = 1280;
const H: i32 = 800;

fn open(desk: &mut Desk, client: u32, activated: bool) -> WindowId {
    desk.open(client, "proc", "Win", Size { w: 400, h: 300 }, activated)
        .0
}

/// Whether this frame's damage covers the whole top bar.
fn bar_damaged(desk: &mut Desk) -> bool {
    let bar = desk.top_bar();
    desk.take_damage()
        .iter()
        .any(|d| d.intersect(&bar) == Some(bar))
}

fn click(desk: &mut Desk, r: Rect) {
    desk.input(Input::Motion(Point {
        x: r.x + r.w / 2,
        y: r.y + r.h / 2,
    }));
    for down in [true, false] {
        desk.input(Input::Button {
            button: Button::Left,
            down,
        });
    }
}

fn fresh() -> Desk {
    let mut desk = Desk::new(W, H);
    desk.take_damage();
    desk
}

#[test]
fn opening_or_closing_a_window_repaints_the_tabs() {
    let mut desk = fresh();
    // Opened by an app, not a person: no focus change, still a new tab.
    let id = open(&mut desk, 1, false);
    assert!(bar_damaged(&mut desk), "a window opened unfocused");
    open(&mut desk, 2, true);
    assert!(bar_damaged(&mut desk), "a window opened with focus");
    desk.close(id);
    assert!(bar_damaged(&mut desk), "a window closed");
}

#[test]
fn a_new_title_focus_order_or_minimize_repaints_the_tabs() {
    let mut desk = fresh();
    let a = open(&mut desk, 1, true);
    let b = open(&mut desk, 2, true);
    desk.take_damage();
    desk.set_title(a, "Notes");
    assert!(bar_damaged(&mut desk), "a title");
    let mut effects: Vec<Effect> = Vec::new();
    desk.set_focus(Focus::Panel, &mut effects);
    assert!(bar_damaged(&mut desk), "focus to the panel");
    // Alt+Tab raises the bottom window, which reorders the tabs.
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
    click(&mut desk, minimize);
    assert!(desk.window(b).unwrap().minimized);
    assert!(bar_damaged(&mut desk), "a minimize");
    desk.activate(b, &mut effects);
    assert!(bar_damaged(&mut desk), "a window back from minimized");
}

#[test]
fn a_commit_alone_leaves_the_tabs_be() {
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

/// Backlog item 9: a press on a minimized window's tab brings it back.
#[test]
fn a_press_on_a_tab_brings_its_window_back() {
    let mut desk = fresh();
    let a = open(&mut desk, 1, true);
    let _b = open(&mut desk, 2, true);
    let mut effects = Vec::new();
    desk.minimize(a, &mut effects);
    assert!(desk.window(a).unwrap().minimized);
    // The painter's tabs, as `desktop` gives them.
    let tab = Rect::new(200, 4, 120, 19);
    desk.set_tabs(alloc::vec![(a, tab)]);
    click(&mut desk, tab);
    let w = desk.window(a).unwrap();
    assert!(!w.minimized, "the tab did not restore it");
    assert_eq!(desk.focus(), Focus::Window(a));
}

/// Backlog item 9: the panel and the top bar take their own clicks, even
/// with a window dragged under them; a dragged title bar stays in reach.
#[test]
fn the_bars_take_their_clicks_over_windows_and_title_bars_stay_in_reach() {
    let mut desk = fresh();
    let a = open(&mut desk, 1, true);
    let frame = desk.window(a).unwrap().frame;
    // Drag it by its title far down, past the panel, and far left.
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
    // A press on the panel focuses the panel, never the window under it.
    let panel = desk.panel();
    click(&mut desk, panel);
    assert_eq!(desk.focus(), Focus::Panel);
}
