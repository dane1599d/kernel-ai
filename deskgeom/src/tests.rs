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
    assert_eq!(l.top_bar, Rect::new(0, 0, 1280, 28));
    assert_eq!(l.panel, Rect::new(0, 764, 1280, 36));
    assert_eq!(l.work_area, Rect::new(0, 28, 1280, 736));
    assert_eq!(l.stats, Rect::new(960, 0, 320, 27));
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
        for r in [l.top_bar, l.panel, l.work_area, l.stats] {
            assert!(inside(screen, r), "{r:?} at {w}x{h}");
        }
        assert_eq!(l.top_bar.bottom(), l.work_area.y);
        assert_eq!(l.work_area.bottom(), l.panel.y);
        assert_eq!(l.panel.bottom(), h);
        assert!(inside(l.top_bar, l.stats));
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
        assert!(hits(l.panel, p.focus_kai()));
        assert!(hits(l.panel, p.pinned()));
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
    assert_eq!(p.focus_kai(), (640, 782));
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
