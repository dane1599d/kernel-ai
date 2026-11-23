//! The wheel for a window's client and the buffer scale: `Client`'s side.

use super::*;

/// The wheel over a window's content, focused or not, is an effect for that
/// window's client at the pointer's place in it; over its title bar, the empty
/// desk, the pane's history or Start it is not.
#[test]
fn the_wheel_over_a_windows_content_is_for_its_client() {
    use crate::chrome::Part;
    use crate::desk::{Hit, Input};
    use crate::geom::{Point, Size};
    let mut desk = Desk::new(1280, 800);
    let (left, _) = desk.open(1, "deskdemo", "Left", Size { w: 300, h: 200 }, true);
    let (right, _) = desk.open(1, "deskdemo", "Right", Size { w: 300, h: 200 }, true);
    assert_eq!(desk.focus(), Focus::Window(right));
    // Over Left's content, where Right (on top, focused) does not reach.
    let on_left = Point { x: 60, y: 200 };
    let Hit::Window(id, Part::Content(at)) = desk.hit(on_left) else {
        panic!("{:?}", desk.hit(on_left));
    };
    assert_eq!(id, left);
    desk.input(Input::Motion(on_left));
    let effects = desk.input(Input::Wheel(2));
    let wheel = Effect::Wheel {
        window: left,
        at,
        detents: 2,
    };
    assert!(effects.contains(&wheel), "{effects:?}");
    // Left was not given the focus by a wheel.
    assert_eq!(desk.focus(), Focus::Window(right));
    // Up is a negative number of detents, and nothing at 0.
    desk.input(Input::Motion(on_left));
    assert!(
        desk.input(Input::Wheel(-1))
            .iter()
            .any(|e| matches!(e, Effect::Wheel { detents: -1, .. }))
    );
    assert!(desk.input(Input::Wheel(0)).is_empty());
    // Not the title bar, not the empty desk.
    for p in [Point { x: 150, y: 60 }, Point { x: 1000, y: 150 }] {
        assert!(
            !matches!(desk.hit(p), Hit::Window(_, Part::Content(_))),
            "{p:?}"
        );
        desk.input(Input::Motion(p));
        let effects = desk.input(Input::Wheel(3));
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::Wheel { .. })),
            "{p:?}: {effects:?}"
        );
    }
}

/// A wheel effect is a ring event of kind `Wheel`: code 0, the detents (held to
/// what a client takes in) and the place in 24.8, with no text.
#[test]
fn a_wheel_goes_to_the_pool_as_an_axis_event() {
    use crate::geom::Point;
    use surfabi::header::Event as Ring;
    use surfabi::input::Kind;
    let mut desk = Desk::new(1280, 800);
    let mut client = Client::new(42, "notes");
    client.handle(&mut desk, &setup(), true).unwrap();
    client.pool_mapped(2);
    let needs = client.handle(&mut desk, &show(), false).unwrap();
    let [Need::Show { window, .. }] = needs[..] else {
        panic!("{needs:?}");
    };
    let at = Point { x: 10, y: 20 };
    let wheel = |detents| Effect::Wheel {
        window,
        at,
        detents,
    };
    client.apply(&[wheel(3), wheel(i32::MIN), wheel(i32::MAX)]);
    let posts = client.take_posts();
    let got: Vec<(u32, Kind, u16, i32, i32, i32, u8)> = posts
        .iter()
        .map(|(pool, e)| match e {
            Ring::Input {
                event, x, y, len, ..
            } => (*pool, event.kind, event.code, event.value, *x, *y, *len),
            other => panic!("{other:?}"),
        })
        .collect();
    let max = crate::server::MAX_DETENTS;
    assert_eq!(
        got,
        [
            (2, Kind::Wheel, 0, 3, 10 << 8, 20 << 8, 0),
            (2, Kind::Wheel, 0, -max, 10 << 8, 20 << 8, 0),
            (2, Kind::Wheel, 0, max, 10 << 8, 20 << 8, 0),
        ]
    );
    // Another window's wheel is not ours.
    client.apply(&[Effect::Wheel {
        window: crate::desk::WindowId(window.0 + 100),
        at,
        detents: 1,
    }]);
    assert!(client.take_posts().is_empty());
}

/// The desktop's preferred scale reaches a client as one registry event, only
/// when it is not 1x.
#[test]
fn a_client_hears_the_scale_only_when_it_is_not_one() {
    let mut client = Client::new(42, "notes");
    events(&mut client);
    client.scale(120);
    assert!(events(&mut client).is_empty());
    client.scale(150);
    let sent = events(&mut client);
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].0, sent[0].1), (REGISTRY, 2));
    assert_eq!(
        Event::decode(Interface::Registry, sent[0].1, &sent[0].2),
        Ok(Event::Scale(150))
    );
}

/// The desktop's whole appearance reaches a client as one more registry event,
/// whole, at any size the saved appearance has.
#[test]
fn a_client_hears_the_whole_appearance_as_one_registry_event() {
    let mut client = Client::new(42, "notes");
    events(&mut client);
    let text = "kai-appearance 1
theme = light
scale = 100
text_size = 150
\n                reduced_motion = off
legible = on
";
    client.look(text);
    let sent = events(&mut client);
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].0, sent[0].1), (REGISTRY, 3));
    assert_eq!(
        Event::decode(Interface::Registry, sent[0].1, &sent[0].2),
        Ok(Event::Look(text))
    );
}

/// A surface at buffer scale 150 with a 300x150 buffer is a 240x120 window,
/// its damage in logical px grown outward, and the desk is told the scale it
/// was drawn at.
#[test]
fn a_buffer_scale_makes_the_window_smaller_in_logical_px() {
    use crate::geom::Size;
    let mut desk = Desk::new(1280, 800);
    let mut client = Client::new(42, "notes");
    let pool = 4096 + 300 * 150 * 4;
    let setup = bytes(&[
        (
            REGISTRY,
            Request::Bind {
                name: 1,
                interface: "pool",
                version: 1,
                id: 2,
            },
        ),
        (
            REGISTRY,
            Request::Bind {
                name: 2,
                interface: "surface",
                version: 1,
                id: 3,
            },
        ),
        (2, Request::PoolCreate { size: pool }),
        (
            2,
            Request::PoolBuffer {
                id: 4,
                offset: 4096,
                width: 300,
                height: 150,
                stride: 1200,
                format: Format::Xrgb8888,
            },
        ),
        (3, Request::GetToplevel { id: 5 }),
    ]);
    client.handle(&mut desk, &setup, true).unwrap();
    client.pool_mapped(2);
    let damage = |x, y, w, h| (3, Request::DamageBuffer(SRect { x, y, w, h }));
    let first = bytes(&[
        (3, Request::SetBufferScale(150)),
        (3, Request::Attach { buffer: 4 }),
        damage(5, 5, 10, 10),
        (3, Request::Commit),
    ]);
    let needs = client.handle(&mut desk, &first, false).unwrap();
    let [Need::Show { window, scale, .. }] = needs[..] else {
        panic!("{needs:?}");
    };
    assert_eq!(scale, 150);
    let content = desk.window(window).unwrap().content();
    assert_eq!(
        Size {
            w: content.w,
            h: content.h
        },
        Size { w: 240, h: 120 }
    );
    // Buffer px 5..15 at 1.25x lie in logical px 4..12.
    desk.take_damage();
    let again = bytes(&[damage(5, 5, 10, 10), (3, Request::Commit)]);
    client.handle(&mut desk, &again, false).unwrap();
    let damaged = desk.take_damage();
    let inside = Rect::new(content.x + 4, content.y + 4, 8, 8);
    assert!(
        damaged.iter().any(|d| d.intersect(&inside) == Some(inside)),
        "{damaged:?} does not hold {inside:?}"
    );
}

/// A surface that never sets a buffer scale is shown at 120.
#[test]
fn a_surface_at_the_default_scale_is_shown_at_it() {
    let mut desk = Desk::new(1280, 800);
    let mut client = Client::new(42, "notes");
    client.handle(&mut desk, &setup(), true).unwrap();
    client.pool_mapped(2);
    let needs = client.handle(&mut desk, &show(), false).unwrap();
    let [Need::Show { scale, .. }] = needs[..] else {
        panic!("{needs:?}");
    };
    assert_eq!(scale, 120);
}

/// A damage rectangle at the extremes of what a client can write (found by
/// `deskcore/tests/hostile.rs`: `Rect::right` overflowed in `logical_damage`)
/// neither panics nor leaves the desk with a rectangle past its screen, at any
/// buffer scale.
#[test]
fn a_damage_rectangle_of_any_size_is_kept_in_bounds() {
    for scale in [120, 150, 360] {
        let mut desk = Desk::new(1280, 800);
        let mut client = Client::new(42, "notes");
        client.handle(&mut desk, &setup(), true).unwrap();
        client.pool_mapped(2);
        let hostile = |x, y, w, h| {
            bytes(&[
                (3, Request::SetBufferScale(scale)),
                (3, Request::Attach { buffer: 4 }),
                (3, Request::DamageBuffer(SRect { x, y, w, h })),
                (3, Request::Commit),
            ])
        };
        for (x, y, w, h) in [
            (0, 0, i32::MAX as u32, i32::MAX as u32),
            (
                i32::MAX as u32,
                i32::MAX as u32,
                i32::MAX as u32,
                i32::MAX as u32,
            ),
            (i32::MAX as u32, 0, i32::MAX as u32, 1),
            (0, i32::MAX as u32, 1, i32::MAX as u32),
        ] {
            // A message the decoder refuses (negative px) closes the connection.
            let _ = client.handle(&mut desk, &hostile(x, y, w, h), false);
            let screen = Rect::new(0, 0, 1280, 800);
            for d in desk.take_damage() {
                assert!(
                    screen.intersect(&d).is_some() || d.w <= 0 || d.h <= 0,
                    "{d:?}"
                );
            }
        }
    }
}
