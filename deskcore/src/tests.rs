use crate::chrome::{self, Part};
use crate::damage::{self, Damage};
use crate::desk::{Button, Desk, Effect, Focus, Hit, Input, WindowId, usage};
use crate::geom::{Point, Rect, Size};
use crate::scale;
use alloc::vec::Vec;

const W: i32 = 1280;
const H: i32 = 800;

fn desk_with(n: usize) -> (Desk, Vec<WindowId>) {
    let mut desk = Desk::new(W, H);
    let ids = (0..n)
        .map(|i| {
            desk.open(i as u32 + 1, "proc", "Win", Size { w: 400, h: 300 }, true)
                .0
        })
        .collect();
    desk.take_damage();
    (desk, ids)
}

fn at(p: (i32, i32)) -> Input {
    Input::Motion(Point { x: p.0, y: p.1 })
}

fn click(desk: &mut Desk, p: (i32, i32)) -> Vec<Effect> {
    let mut effects = desk.input(at(p));
    effects.extend(desk.input(Input::Button {
        button: Button::Left,
        down: true,
    }));
    effects.extend(desk.input(Input::Button {
        button: Button::Left,
        down: false,
    }));
    effects
}

fn key(desk: &mut Desk, code: u16, down: bool) -> Vec<Effect> {
    desk.input(Input::Key { usage: code, down })
}

#[test]
fn rects_intersect_union_and_contain() {
    let a = Rect::new(0, 0, 10, 10);
    let b = Rect::new(5, 5, 10, 10);
    assert_eq!(a.intersect(&b), Some(Rect::new(5, 5, 5, 5)));
    assert_eq!(a.union(&b), Rect::new(0, 0, 15, 15));
    assert_eq!(a.intersect(&Rect::new(10, 0, 5, 5)), None);
    assert!(a.contains(Point { x: 9, y: 9 }) && !a.contains(Point { x: 10, y: 0 }));
    assert_eq!(Rect::default().union(&b), b);
}

#[test]
fn damage_merges_cuts_and_caps_at_sixteen() {
    let mut d = Damage::new(100, 100);
    d.add(Rect::new(0, 0, 10, 10));
    d.add(Rect::new(2, 2, 3, 3));
    d.add(Rect::new(-5, 95, 20, 20));
    assert_eq!(d.take(), [Rect::new(0, 0, 10, 10), Rect::new(0, 95, 15, 5)]);
    // Overlapping neighbours merge when the union is tight.
    d.add(Rect::new(0, 0, 10, 10));
    d.add(Rect::new(5, 0, 10, 10));
    assert_eq!(d.take(), [Rect::new(0, 0, 15, 10)]);
    for n in 0..20 {
        d.add(Rect::new(n * 5, n * 5, 2, 2));
    }
    assert_eq!(d.take(), [Rect::new(0, 0, 97, 97)]);
    assert!(d.take().is_empty());
    const { assert!(damage::MAX == 16) };
}

#[test]
fn chrome_hits_controls_title_content_and_grabs() {
    let content = Rect::new(100, 100, 400, 300);
    let frame = chrome::frame_for(content);
    assert_eq!(chrome::content_of(frame), content);
    let p = |x, y| Point { x, y };
    assert_eq!(chrome::hit(frame, p(150, 80)), Some(Part::Title));
    assert_eq!(
        chrome::hit(frame, p(110, 150)),
        Some(Part::Content(p(10, 50)))
    );
    let [(close, c), (max, m), (min, n)] = chrome::controls(frame);
    assert_eq!(
        (close, max, min),
        (Part::Close, Part::Maximize, Part::Minimize)
    );
    assert_eq!(chrome::hit(frame, p(c.x + 1, c.y + 1)), Some(Part::Close));
    assert_eq!(
        chrome::hit(frame, p(m.x + 1, m.y + 1)),
        Some(Part::Maximize)
    );
    assert_eq!(
        chrome::hit(frame, p(n.x + 1, n.y + 1)),
        Some(Part::Minimize)
    );
    assert!(c.right() <= frame.right() && n.x > m.x - chrome::CONTROL - 3);
    // 6 px outside the frame is a grab; 7 px is nothing.
    let Some(Part::Edge(e)) = chrome::hit(frame, p(frame.right() + 5, 250)) else {
        panic!("no right edge");
    };
    assert!(e.right && !e.left && !e.top && !e.bottom);
    assert_eq!(chrome::hit(frame, p(frame.right() + 6, 250)), None);
    // Near a corner the grab takes both edges.
    let Some(Part::Edge(e)) = chrome::hit(frame, p(frame.right() + 2, frame.bottom() - 5)) else {
        panic!("no corner");
    };
    assert!(e.right && e.bottom);
}

#[test]
fn a_window_opens_in_the_work_area_and_takes_focus_only_when_activated() {
    let mut desk = Desk::new(W, H);
    let (a, effects) = desk.open(1, "notes", "Notes", Size { w: 400, h: 300 }, true);
    assert_eq!(effects, [Effect::Focus(Focus::Window(a))]);
    let (b, effects) = desk.open(2, "spam", "Popup", Size { w: 200, h: 100 }, false);
    assert!(
        effects.is_empty(),
        "a window no user made does not steal focus"
    );
    assert_eq!(desk.focus(), Focus::Window(a));
    let area = desk.work_area();
    for id in [a, b] {
        let frame = desk.window(id).unwrap().frame;
        assert!(
            area.intersect(&frame) == Some(frame),
            "{frame:?} outside {area:?}"
        );
    }
    assert!(desk.take_damage().contains(&Rect::new(0, 0, W, H)));
}

#[test]
fn a_click_raises_and_focuses_and_its_content_gets_the_button() {
    let (mut desk, ids) = desk_with(2);
    let lower = desk.window(ids[0]).unwrap().content();
    // A point in the lower window that the upper one does not cover.
    let p = (lower.x + 5, lower.y + 5);
    let effects = click(&mut desk, p);
    assert!(effects.contains(&Effect::Focus(Focus::Window(ids[0]))));
    assert!(effects.contains(&Effect::Button {
        window: ids[0],
        at: Point { x: 5, y: 5 },
        button: Button::Left,
        down: true
    }));
    assert_eq!(desk.windows().last().unwrap().id, ids[0]);
    assert!(!desk.take_damage().is_empty());
}

#[test]
fn the_title_bar_drags_a_window_200_px_and_damages_both_places() {
    let (mut desk, ids) = desk_with(1);
    let frame = desk.window(ids[0]).unwrap().frame;
    let grab = (frame.x + 60, frame.y + 10);
    desk.input(at(grab));
    desk.input(Input::Button {
        button: Button::Left,
        down: true,
    });
    desk.take_damage();
    assert_eq!(desk.dragged(), Some(ids[0]));
    desk.input(at((grab.0 + 200, grab.1)));
    let moved = desk.window(ids[0]).unwrap().frame;
    assert_eq!(moved, frame.translate(200, 0));
    let damage = desk.take_damage();
    let covers = |r: Rect| damage.iter().any(|d| d.intersect(&r) == Some(r));
    assert!(covers(frame) && covers(moved), "{damage:?}");
    desk.input(Input::Button {
        button: Button::Left,
        down: false,
    });
    let released = desk.take_damage();
    assert!(
        released.iter().any(|d| d.intersect(&moved) == Some(moved)),
        "{released:?}"
    );
    desk.input(at((10, 400)));
    assert_eq!(
        desk.window(ids[0]).unwrap().frame,
        moved,
        "released: no more moving"
    );
    // While it moved it was the dragged window; let go, it is not, and its
    // place is damaged so its shadow is drawn again.
    assert_eq!(desk.dragged(), None);
    // Dragged up, the title bar stops at the top of the work area.
    let grab = (moved.x + 60, moved.y + 10);
    desk.input(at(grab));
    desk.input(Input::Button {
        button: Button::Left,
        down: true,
    });
    desk.input(at((grab.0, 0)));
    assert_eq!(desk.window(ids[0]).unwrap().frame.y, desk.work_area().y);
}

#[test]
fn an_edge_resizes_through_the_client() {
    let (mut desk, ids) = desk_with(1);
    let frame = desk.window(ids[0]).unwrap().frame;
    desk.input(at((frame.right() + 3, frame.y + 100)));
    desk.input(Input::Button {
        button: Button::Left,
        down: true,
    });
    let effects = desk.input(at((frame.right() + 53, frame.y + 100)));
    assert_eq!(
        effects,
        [Effect::Configure {
            window: ids[0],
            size: Size { w: 450, h: 300 },
            maximized: false,
            activated: true
        }]
    );
    // The frame changes when the client commits the new size.
    assert_eq!(desk.window(ids[0]).unwrap().frame, frame);
    desk.commit(ids[0], Size { w: 450, h: 300 }, &[]);
    assert_eq!(desk.window(ids[0]).unwrap().frame.w, frame.w + 50);
    // Past the minimum, the size stops there.
    let effects = desk.input(at((0, frame.y + 100)));
    assert!(matches!(
        effects[..],
        [Effect::Configure {
            size: Size { w: 120, .. },
            ..
        }]
    ));
}

#[test]
fn controls_minimize_maximize_and_ask_to_close() {
    let (mut desk, ids) = desk_with(2);
    let top = ids[1];
    let frame = desk.window(top).unwrap().frame;
    let [(_, close), (_, max), _] = chrome::controls(frame);
    let centre = |r: Rect| (r.x + r.w / 2, r.y + r.h / 2);
    assert!(click(&mut desk, centre(close)).contains(&Effect::Close { window: top }));
    let effects = click(&mut desk, centre(max));
    let area = desk.work_area();
    let want = chrome::content_of(area);
    assert!(effects.contains(&Effect::Configure {
        window: top,
        size: Size {
            w: want.w,
            h: want.h
        },
        maximized: true,
        activated: true
    }));
    assert!(desk.window(top).unwrap().maximized());
    desk.commit(
        top,
        Size {
            w: want.w,
            h: want.h,
        },
        &[],
    );
    let frame = desk.window(top).unwrap().frame;
    let [_, (_, max), _] = chrome::controls(frame);
    let effects = click(&mut desk, centre(max));
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::Configure {
            maximized: false,
            ..
        }
    )));
    desk.commit(top, Size { w: 400, h: 300 }, &[]);
    let frame = desk.window(top).unwrap().frame;
    let [_, _, (_, min)] = chrome::controls(frame);
    let effects = click(&mut desk, centre(min));
    assert!(desk.window(top).unwrap().minimized);
    assert!(effects.contains(&Effect::Focus(Focus::Window(ids[0]))));
    // Minimized, it is not hit.
    assert!(
        !matches!(desk.hit(Point { x: frame.x + 5, y: frame.y + 5 }), Hit::Window(id, _) if id == top)
    );
}

#[test]
fn keys_go_to_the_focused_window_and_shortcuts_go_to_the_desk() {
    let (mut desk, ids) = desk_with(2);
    assert_eq!(
        key(&mut desk, 0x04, true),
        [Effect::Key {
            window: ids[1],
            usage: 0x04,
            down: true
        }]
    );
    // Alt+Tab: the other window rises and takes the keys.
    key(&mut desk, usage::LEFT_ALT, true);
    let effects = key(&mut desk, usage::TAB, true);
    assert!(effects.contains(&Effect::Focus(Focus::Window(ids[0]))));
    assert!(key(&mut desk, usage::TAB, false).is_empty());
    key(&mut desk, usage::LEFT_ALT, false);
    assert_eq!(desk.windows().last().unwrap().id, ids[0]);
    // Ctrl+Space and a lone Super focus the panel; Super with a key does not.
    key(&mut desk, usage::LEFT_CTRL, true);
    assert_eq!(
        key(&mut desk, usage::SPACE, true),
        [Effect::Focus(Focus::Panel)]
    );
    key(&mut desk, usage::SPACE, false);
    key(&mut desk, usage::LEFT_CTRL, false);
    assert_eq!(
        key(&mut desk, 0x05, true),
        [Effect::PanelKey {
            usage: 0x05,
            down: true
        }]
    );
    let c = desk.window(ids[0]).unwrap().content();
    click(&mut desk, (c.x + 2, c.y + 2));
    key(&mut desk, usage::LEFT_GUI, true);
    key(&mut desk, 0x06, true);
    assert!(!key(&mut desk, usage::LEFT_GUI, false).contains(&Effect::Focus(Focus::Panel)));
    key(&mut desk, usage::LEFT_GUI, true);
    assert_eq!(
        key(&mut desk, usage::LEFT_GUI, false),
        [Effect::Focus(Focus::Panel)]
    );
}

#[test]
fn closing_the_focused_window_focuses_the_next_and_damages_its_place() {
    let (mut desk, ids) = desk_with(3);
    let frame = desk.window(ids[2]).unwrap().frame;
    let effects = desk.close(ids[2]);
    assert_eq!(effects, [Effect::Focus(Focus::Window(ids[1]))]);
    assert!(
        desk.take_damage()
            .iter()
            .any(|d| d.intersect(&frame) == Some(frame))
    );
    desk.close(ids[1]);
    assert_eq!(desk.close(ids[0]), [Effect::Focus(Focus::Nothing)]);
    assert!(desk.close(ids[0]).is_empty(), "closing twice is harmless");
}

#[test]
fn a_commit_damages_only_what_the_client_drew() {
    let (mut desk, ids) = desk_with(1);
    let content = desk.window(ids[0]).unwrap().content();
    desk.commit(
        ids[0],
        Size { w: 400, h: 300 },
        &[Rect::new(10, 20, 30, 40), Rect::new(390, 290, 50, 50)],
    );
    assert_eq!(
        desk.take_damage(),
        [
            Rect::new(content.x + 10, content.y + 20, 30, 40),
            Rect::new(content.x + 390, content.y + 290, 10, 10)
        ]
    );
}

#[test]
fn pointer_enters_and_leaves_content() {
    let (mut desk, ids) = desk_with(1);
    let c = desk.window(ids[0]).unwrap().content();
    assert_eq!(
        desk.input(at((c.x + 3, c.y + 4))),
        [Effect::Pointer {
            window: ids[0],
            at: Point { x: 3, y: 4 }
        }]
    );
    assert_eq!(
        desk.input(at((c.x + 3, c.y - 10))),
        [Effect::Leave { window: ids[0] }]
    );
    // Off the screen, the pointer stays on its edge.
    desk.input(at((-50, 5000)));
    assert_eq!(desk.pointer(), Point { x: 0, y: H - 1 });
}

#[test]
fn the_scale_follows_the_edid_or_the_width() {
    assert_eq!(scale::for_screen(1920, 0), 120);
    assert_eq!(scale::for_screen(2560, 0), 150);
    assert_eq!(scale::for_screen(3840, 0), 240);
    // A 27-inch 4K screen is about 597 mm wide: 163 px per inch, so 1.75.
    assert_eq!(scale::for_screen(3840, 597), 210);
    // A 24-inch 1080p screen, about 531 mm: 92 px per inch, so 1.0.
    assert_eq!(scale::for_screen(1920, 531), 120);
    // Clamped to 1.0..3.0.
    assert_eq!(scale::for_screen(800, 600), 120);
    assert_eq!(scale::for_screen(7680, 200), 360);
    assert_eq!(scale::to_physical(100, 150), 125);
    assert_eq!(scale::to_logical(125, 150), 100);
}

fn centre(r: Rect) -> (i32, i32) {
    (r.x + r.w / 2, r.y + r.h / 2)
}

#[test]
fn an_ask_card_takes_only_y_n_or_a_click_and_drops_keys_held_before_it() {
    let (mut desk, ids) = desk_with(2);
    // `a` is held down when the card appears.
    key(&mut desk, 0x04, true);
    assert_eq!(desk.show_ask(7, 3), [Effect::Focus(Focus::Ask(7))]);
    assert!(desk.take_damage().iter().any(|d| {
        let card = crate::trusted::card(W, H, &desk.ask().unwrap());
        d.intersect(&card) == Some(card)
    }));
    // Its release, Enter, Esc and other keys go nowhere; no window hears them.
    for code in [0x04, 0x28, 0x29, 0x05] {
        assert!(key(&mut desk, code, true).is_empty() || code == 0x04);
        assert!(key(&mut desk, code, false).is_empty());
    }
    // A click on a window neither raises it nor reaches it.
    let lower = desk.window(ids[0]).unwrap().content();
    assert!(click(&mut desk, (lower.x + 3, lower.y + 3)).is_empty());
    assert_eq!(desk.windows().last().unwrap().id, ids[1]);
    // Shortcuts cannot take focus from it.
    key(&mut desk, usage::LEFT_CTRL, true);
    assert!(key(&mut desk, usage::SPACE, true).is_empty());
    key(&mut desk, usage::SPACE, false);
    key(&mut desk, usage::LEFT_CTRL, false);
    assert_eq!(desk.focus(), Focus::Ask(7));
    // y answers yes; focus goes back to the window.
    assert_eq!(
        key(&mut desk, usage::Y, true),
        [
            Effect::Answer { id: 7, yes: true },
            Effect::Focus(Focus::Window(ids[1]))
        ]
    );
    assert_eq!(desk.ask(), None);
}

#[test]
fn a_y_held_before_the_card_cannot_answer_it_and_buttons_can() {
    let (mut desk, _) = desk_with(1);
    key(&mut desk, usage::Y, true);
    desk.show_ask(9, 1);
    // The y still held repeats or comes up: neither answers.
    assert!(key(&mut desk, usage::Y, true).is_empty());
    assert!(key(&mut desk, usage::Y, false).is_empty());
    let card = crate::trusted::card(W, H, &desk.ask().unwrap());
    let (_, no) = crate::trusted::buttons(card);
    let effects = click(&mut desk, centre(no));
    assert!(effects.contains(&Effect::Answer { id: 9, yes: false }));
    // A card that times out goes without an answer.
    desk.show_ask(10, 1);
    assert_eq!(desk.hide_ask(11), []);
    assert!(!desk.hide_ask(10).is_empty());
    assert_eq!(desk.ask(), None);
}

#[test]
fn a_key_or_click_in_the_agents_window_takes_over() {
    let (mut desk, ids) = desk_with(2);
    desk.set_agent(Some(ids[1]));
    desk.take_damage();
    // A key while another window has focus is not taking over.
    let other = desk.window(ids[0]).unwrap().content();
    click(&mut desk, (other.x + 2, other.y + 2));
    assert_eq!(desk.agent(), Some(ids[1]));
    // A click on the agent's window is, where the other window does not cover it.
    let agent = desk.window(ids[1]).unwrap().content();
    let effects = click(&mut desk, (agent.right() - 3, agent.bottom() - 3));
    assert!(effects.contains(&Effect::TookOver { window: ids[1] }));
    assert_eq!(desk.agent(), None);
    let outline = crate::trusted::outline(desk.window(ids[1]).unwrap().frame);
    assert!(
        desk.take_damage()
            .iter()
            .any(|d| d.intersect(&outline) == Some(outline))
    );
    // A key in the focused agent window is too.
    desk.set_agent(Some(ids[1]));
    assert!(key(&mut desk, 0x04, true).contains(&Effect::TookOver { window: ids[1] }));
}
