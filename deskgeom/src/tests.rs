use crate::*;

/// The sizes the desk is designed and tested at.
const SIZES: [(i32, i32); 3] = [(1920, 1080), (1280, 800), (1024, 768)];
/// Notes' frame: its 640 px content plus the frame.
const NOTES_W: i32 = 642;

fn layout(w: i32, h: i32) -> Layout {
    Layout::for_screen(w, h, UiState::default())
}

fn inside(outer: Rect, inner: Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.right() <= outer.right()
        && inner.bottom() <= outer.bottom()
}

fn hits(r: Rect, p: (i32, i32)) -> bool {
    r.contains(points::point(p))
}

#[test]
fn todays_desk_keeps_todays_numbers() {
    let l = layout(1280, 800);
    assert_eq!(l.work_area, Rect::new(0, 0, 1280, 752));
    assert_eq!(l.taskbar, Rect::new(0, 752, 1280, 48));
    assert_eq!(l.maximized, l.work_area);
    assert_eq!(l.cascade(0), Point { x: 48, y: 52 });
    assert_eq!(l.cascade(1), Point { x: 80, y: 84 });
    assert_eq!(l.cascade(8), l.cascade(0));
    let frame = Rect::new(48, 52, NOTES_W, 454);
    let c = controls(frame);
    assert_eq!(c.close, Rect::new(649, 53, 40, 32));
    assert_eq!(c.maximize, Rect::new(609, 53, 40, 32));
    assert_eq!(c.minimize, Rect::new(569, 53, 40, 32));
    assert_eq!(content_of(frame), Rect::new(49, 85, 640, 420));
    assert_eq!(frame_for(content_of(frame)), frame);
}

#[test]
fn every_surface_is_on_the_screen_and_they_tile_its_height() {
    for (w, h) in SIZES {
        let l = layout(w, h);
        let screen = Rect::new(0, 0, w, h);
        for r in [l.work_area, l.taskbar, l.tray.area, l.pane] {
            assert!(inside(screen, r), "{r:?} at {w}x{h}");
        }
        assert_eq!(l.work_area.y, 0);
        assert_eq!(l.work_area.bottom(), l.taskbar.y);
        assert_eq!(l.taskbar.bottom(), h);
        assert!(inside(l.taskbar, l.tray.area));
        for n in 0..CASCADE_SPOTS {
            assert!(inside(
                l.work_area,
                points(w, h).frame(n, Size { w: 302, h: 268 })
            ));
        }
    }
}

#[test]
fn each_point_lands_in_its_target() {
    for (w, h) in SIZES {
        let p = points(w, h);
        let l = p.layout;
        assert!(hits(l.tray.corner, p.focus_kai()));
        assert!(hits(l.pane_parts().input, p.pane_input()));
        assert!(hits(l.work_area, p.desk_point()));
        assert!(hits(l.work_area, p.screen_centre()));
        for n in 0..CASCADE_SPOTS {
            let frame = p.frame(n, Size { w: NOTES_W, h: 454 });
            let c = controls(frame);
            assert!(hits(c.close, p.close(n, NOTES_W)));
            assert!(hits(c.maximize, p.maximize(n, NOTES_W)));
            assert!(hits(c.minimize, p.minimize(n, NOTES_W)));
            let title = p.window_title(n, 60);
            assert!(hits(title_bar(frame), title));
            // The next spot's window never covers it (past the last spot,
            // the next window starts over at spot 0).
            if n + 1 < CASCADE_SPOTS {
                assert!(!hits(p.frame(n + 1, Size { w: NOTES_W, h: 454 }), title));
            }
            assert!(hits(content_of(frame), p.content(n, 0, 0)));
            assert!(!hits(content_of(frame), p.content(n, -1, -1)));
        }
    }
}

#[test]
fn the_desk_point_is_clear_of_every_window_and_its_grabs() {
    for (w, h) in SIZES {
        let p = points(w, h);
        for n in 0..CASCADE_SPOTS {
            let frame = p.frame(n, Size { w: w * 2, h: h * 2 });
            assert!(!hits(frame.grow(GRAB), p.desk_point()));
        }
        assert_ne!(p.desk_point(), p.screen_centre());
    }
}

#[test]
fn the_selftests_points_hit_what_they_hit() {
    let p = points(1280, 800);
    assert_eq!(p.screen_centre(), (640, 400));
    assert_eq!(p.focus_kai(), (1279, 799));
    assert_eq!(p.pane_input(), (908 + 180, 12 + 728 - 12 - 22));
    assert_eq!(p.kai_button(7), (508, 776));
    // Kai Desk's 40x32 controls, flush: the close control's middle.
    assert_eq!(p.close(0, NOTES_W), (48 + 642 - 1 - 20, 52 + 1 + 16));
    // The old close point is still on close; the old minimize point is on
    // maximize now, so the selftests click points' minimize, not a number.
    let frame = p.frame(0, Size { w: NOTES_W, h: 454 });
    assert!(hits(controls(frame).close, (48 + 642 - 17, 52 + 17)));
    assert!(hits(controls(frame).maximize, (48 + 642 - 76, 52 + 17)));
    assert_eq!(p.content(0, 320, 211), (48 + 321, 52 + 34 + 210));
    assert_eq!(p.window_title(1, 60), (140, 92));
    assert_eq!(p.window_title(2, 388), (500, 124));
    assert!(hits(controls(frame).minimize, p.minimize(0, NOTES_W)));
}

/// ADR 0005 §8's pane table, and its parts inside it and apart.
#[test]
fn the_pane_is_where_the_adr_puts_it() {
    let want = [
        ((1920, 1080), Rect::new(1508, 12, 400, 1008)),
        ((1280, 800), Rect::new(908, 12, 360, 728)),
        ((1024, 768), Rect::new(672, 12, 340, 696)),
    ];
    for ((w, h), pane) in want {
        let l = layout(w, h);
        assert_eq!(l.pane, pane);
        let p = l.pane_parts();
        for part in [
            p.header, p.mark, p.close, p.history, p.status, p.stop, p.input, p.send,
        ] {
            assert!(inside(pane, part), "{part:?} at {w}x{h}");
        }
        assert!(inside(p.header, p.close) && inside(p.header, p.mark));
        assert!(inside(p.status, p.stop) && inside(p.input, p.send));
        // Top to bottom, never overlapping: header, history, status, input.
        assert!(p.header.bottom() <= p.history.y);
        assert!(p.history.bottom() <= p.status.y);
        assert!(p.status.bottom() <= p.input.y);
        assert_eq!(
            p.input,
            Rect::new(pane.x + 12, pane.bottom() - 56, pane.w - 24, 44)
        );
        // 12 above the taskbar.
        assert_eq!(pane.bottom(), h - TASKBAR - FLYOUT_MARGIN);
    }
}

/// The card's anchor: bottom at H-60, as tall as the work height, H-72.
#[test]
fn the_card_sits_above_the_taskbar() {
    for ((w, h), max_h) in [((1920, 1080), 1008), ((1280, 800), 728), ((1024, 768), 696)] {
        let l = layout(w, h);
        assert_eq!(l.card_bottom, h - 60);
        assert_eq!(l.card_max_h, max_h);
    }
}

/// With the pane open at 1280 and up, a maximized window fits beside it;
/// under 1280 the pane is a flyout and maximize fills the work area.
#[test]
fn maximized_windows_fit_beside_an_open_pane() {
    for (w, h) in SIZES {
        let open = Layout::for_screen(w, h, UiState { pane_open: true });
        let closed = layout(w, h);
        assert_eq!(closed.maximized, closed.work_area);
        match w >= PANE_BESIDE {
            true => {
                assert_eq!(open.maximized.right(), open.pane.x - FLYOUT_MARGIN);
                assert_eq!(open.maximized.y, open.work_area.y);
                assert_eq!(open.maximized.h, open.work_area.h);
            }
            false => assert_eq!(open.maximized, open.work_area),
        }
    }
}

/// ADR 0005 §8's taskbar table: a 7-item group and the Kai button's centre.
#[test]
fn the_taskbar_group_is_where_the_adr_puts_it() {
    let want = [
        ((1920, 1080), (808, 1112), (828, 1056)),
        ((1280, 800), (488, 792), (508, 776)),
        ((1024, 768), (360, 664), (380, 744)),
    ];
    for ((w, h), (x0, x1), centre) in want {
        let first = taskbar::item(w, h, 0, 7);
        let last = taskbar::item(w, h, 6, 7);
        assert_eq!((first.x, last.right()), (x0, x1), "{w}x{h}");
        assert_eq!(points(w, h).kai_button(7), centre);
        assert_eq!(first.y, h - 44);
    }
}

/// ADR 0005 §8's tray table, and the compact tray under 1280.
#[test]
fn the_tray_is_where_the_adr_puts_it() {
    let t = taskbar::tray(1920, 1080);
    assert_eq!(t.corner, Rect::new(1904, 1032, 16, 48));
    assert_eq!(t.clock, Rect::new(1824, 1032, 80, 48));
    assert_eq!(t.memory, Some(Rect::new(1728, 1036, 88, 40)));
    assert_eq!(t.chip, Rect::new(1560, 1036, 160, 40));
    assert_eq!(t.area, Rect::new(1552, 1032, 368, 48));
    assert_eq!(taskbar::tray(1280, 800).area.x, 912);
    let c = taskbar::tray(1024, 768);
    assert_eq!(c.memory, None);
    assert_eq!(c.chip, Rect::new(856, 724, 64, 40));
    assert_eq!(c.area, Rect::new(848, 720, 176, 48));
    for (w, h) in SIZES {
        let t = taskbar::tray(w, h);
        // The corner holds the screen's last pixel; the clock is on 16 px
        // columns; nothing in the tray overlaps.
        assert!(hits(t.corner, (w - 1, h - 1)));
        assert_eq!(t.clock.x % 16, 0);
        assert_eq!(t.clock.right() % 16, 0);
        assert!(t.chip.right() <= t.memory.map_or(t.clock.x, |m| m.x));
        assert!(t.memory.is_none_or(|m| m.right() <= t.clock.x));
        assert!(t.clock.right() <= t.corner.x);
        assert!(inside(t.area, t.chip) && inside(t.area, t.clock));
    }
}

/// The group never meets the tray, at the most items each size allows, and
/// each item's hit box covers the whole taskbar height with no dead pixel
/// between neighbours.
#[test]
fn the_group_never_meets_the_tray() {
    for (w, h) in SIZES {
        let n = taskbar::max_items(w) as i32;
        let last = taskbar::item(w, h, n - 1, n);
        assert!(
            last.right() + taskbar::GROUP_MARGIN <= taskbar::tray(w, h).area.x,
            "{w}x{h}"
        );
        assert!(taskbar::item(w, h, 0, n).x >= taskbar::GROUP_MARGIN);
        for i in 0..n - 1 {
            let (a, b) = (
                taskbar::item_hit(w, h, i, n),
                taskbar::item_hit(w, h, i + 1, n),
            );
            assert_eq!(a.right(), b.x, "{w}x{h} item {i}");
            assert_eq!((a.y, a.h), (h - TASKBAR, TASKBAR));
        }
    }
    assert_eq!(
        (taskbar::max_items(1920), taskbar::max_items(1024)),
        (12, 9)
    );
}
