use crate::compose::{Pixels, Source, over};
use crate::desk::{Desk, Effect, Focus};
use crate::geom::Rect;
use crate::server::{Client, Need};
use alloc::vec;
use alloc::vec::Vec;
use surfabi::proto::{Event, Interface, REGISTRY, Request};
use surfabi::wire::{self, Error};
use surfabi::{Format, Rect as SRect};

fn bytes(requests: &[(u32, Request)]) -> Vec<u8> {
    requests
        .iter()
        .flat_map(|(object, request)| request.encode(*object).unwrap())
        .collect()
}

const POOL: u32 = 4096 + 64 * 32 * 4;

/// Binds a pool and a surface, makes a 64x32 buffer and a titled toplevel.
fn setup() -> Vec<u8> {
    bytes(&[
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
        (
            REGISTRY,
            Request::Bind {
                name: 3,
                interface: "seat",
                version: 1,
                id: 7,
            },
        ),
        (2, Request::PoolCreate { size: POOL }),
        (
            2,
            Request::PoolBuffer {
                id: 4,
                offset: 4096,
                width: 64,
                height: 32,
                stride: 256,
                format: Format::Xrgb8888,
            },
        ),
        (3, Request::GetToplevel { id: 5 }),
        (5, Request::SetTitle("Demo")),
    ])
}

fn show() -> Vec<u8> {
    bytes(&[
        (3, Request::Attach { buffer: 4 }),
        (
            3,
            Request::DamageBuffer(SRect {
                x: 0,
                y: 0,
                w: 64,
                h: 32,
            }),
        ),
        (3, Request::Commit),
    ])
}

fn events(client: &mut Client) -> Vec<(u32, u16, Vec<u8>)> {
    let all = client.take_events(4096);
    let mut out = Vec::new();
    let mut rest = &all[..];
    while let Ok((h, args, next)) = wire::split(rest) {
        out.push((h.object, h.opcode, args.to_vec()));
        rest = next;
    }
    out
}

#[test]
fn a_client_opens_a_window_once_its_pool_is_mapped() {
    let mut desk = Desk::new(1280, 800);
    let mut client = Client::new(42, "deskdemo");
    // The registry announced every global.
    let globals = events(&mut client);
    assert_eq!(globals.len(), 4);
    assert_eq!(
        Event::decode(Interface::Registry, globals[0].1, &globals[0].2),
        Ok(Event::Global {
            name: 1,
            interface: "pool",
            version: 1
        })
    );
    let needs = client.handle(&mut desk, &setup(), true).unwrap();
    assert_eq!(
        needs,
        [Need::MapPool {
            pool: 2,
            size: POOL
        }]
    );
    // A commit in the same call as pool.create shows once the pool is mapped:
    // the needs come in order.
    let mut together = Client::new(43, "deskdemo");
    let needs = together
        .handle(&mut Desk::new(1280, 800), &[setup(), show()].concat(), true)
        .unwrap();
    assert!(
        matches!(needs[..], [Need::MapPool { .. }, Need::Show { .. }]),
        "{needs:?}"
    );
    let needs = client.handle(&mut desk, &show(), false).unwrap();
    let [Need::Show { window, buffer }] = needs[..] else {
        panic!("{needs:?}");
    };
    assert_eq!((buffer.width, buffer.height, buffer.offset), (64, 32, 4096));
    let w = desk.window(window).unwrap();
    assert_eq!((w.title.as_str(), w.label.as_str()), ("Demo", "deskdemo"));
    assert_eq!(desk.focus(), Focus::Window(window));
    // The seat heard about the focus.
    let sent = events(&mut client);
    assert!(
        sent.iter().any(|(object, op, _)| *object == 7 && *op == 0),
        "{sent:?}"
    );
}

#[test]
fn a_handle_only_comes_with_pool_create_and_buffers_stay_in_their_pool() {
    let mut desk = Desk::new(1280, 800);
    let mut client = Client::new(1, "x");
    let create = bytes(&[
        (
            REGISTRY,
            Request::Bind {
                name: 1,
                interface: "pool",
                version: 1,
                id: 2,
            },
        ),
        (2, Request::PoolCreate { size: POOL }),
    ]);
    assert_eq!(client.handle(&mut desk, &create, false), Err(Error::BadArg));
    let mut client = Client::new(1, "x");
    let twice = [
        create.clone(),
        bytes(&[(2, Request::PoolCreate { size: POOL })]),
    ]
    .concat();
    assert_eq!(client.handle(&mut desk, &twice, true), Err(Error::BadArg));
    let mut client = Client::new(1, "x");
    client.handle(&mut desk, &create, true).unwrap();
    let past = bytes(&[(
        2,
        Request::PoolBuffer {
            id: 4,
            offset: 4096,
            width: 64,
            height: 33,
            stride: 256,
            format: Format::Xrgb8888,
        },
    )]);
    assert_eq!(client.handle(&mut desk, &past, false), Err(Error::BadArg));
    let header = bytes(&[(
        2,
        Request::PoolBuffer {
            id: 4,
            offset: 0,
            width: 4,
            height: 1,
            stride: 16,
            format: Format::Xrgb8888,
        },
    )]);
    assert_eq!(client.handle(&mut desk, &header, false), Err(Error::BadArg));
}

#[test]
fn configure_close_and_destroy_go_through_the_client() {
    let mut desk = Desk::new(1280, 800);
    let mut client = Client::new(9, "demo");
    client.handle(&mut desk, &setup(), true).unwrap();
    client.pool_mapped(2);
    let Need::Show { window, .. } = client.handle(&mut desk, &show(), false).unwrap()[0] else {
        panic!();
    };
    events(&mut client);
    let size = crate::geom::Size { w: 300, h: 200 };
    client.apply(&[
        Effect::Configure {
            window,
            size,
            maximized: true,
            activated: true,
        },
        Effect::Close { window },
    ]);
    let sent = events(&mut client);
    let configure = Event::decode(Interface::Toplevel, sent[0].1, &sent[0].2).unwrap();
    let Event::Configure {
        width,
        height,
        states,
        ..
    } = configure
    else {
        panic!("{configure:?}");
    };
    assert_eq!((width, height), (300, 200));
    assert_eq!(states, [1, 0, 0, 0, 4, 0, 0, 0]);
    assert_eq!(
        Event::decode(Interface::Toplevel, sent[1].1, &sent[1].2),
        Ok(Event::Close)
    );
    // The client destroys its toplevel: the window goes.
    let needs = client
        .handle(&mut desk, &bytes(&[(5, Request::ToplevelDestroy)]), false)
        .unwrap();
    assert_eq!(needs, [Need::Closed { window }]);
    assert!(desk.window(window).is_none());
    assert_eq!(client.windows().count(), 0);
}

#[test]
fn events_are_sent_in_whole_messages() {
    let mut client = Client::new(1, "x");
    let all = client.take_events(30);
    assert!(all.len() <= 30 && wire::split(&all).is_ok());
    assert!(!client.take_events(4096).is_empty());
    assert!(client.take_events(4096).is_empty());
}

#[test]
fn fills_and_draws_are_cut_and_blend_premultiplied() {
    let mut frame = vec![0u32; 8 * 4];
    let mut px = Pixels {
        data: &mut frame,
        width: 8,
        height: 4,
        stride: 8,
    };
    px.fill(Rect::new(-2, -2, 4, 4), Rect::new(0, 0, 8, 4), 0xFF11_2233);
    assert_eq!(&px.data[..3], &[0xFF11_2233, 0xFF11_2233, 0]);
    let src = [0x8040_0000u32; 4];
    let source = Source {
        data: &src,
        width: 2,
        height: 2,
        stride: 2,
        alpha: true,
    };
    px.draw(&source, (6, 3), Rect::new(0, 0, 8, 4));
    // Cut at the frame's edge: only the top row lands.
    assert_eq!(px.data[3 * 8 + 6], over(0x8040_0000, 0));
    assert_eq!(over(0xFF12_3456, 0xFF00_0000), 0xFF12_3456);
    assert_eq!(over(0x0000_0000, 0xFF10_2030), 0xFF10_2030);
    // Half-transparent black over white: grey, opaque.
    assert_eq!(over(0x8000_0000, 0xFFFF_FFFF), 0xFF7F_7F7F);
}

/// The numbers `test=desk-windows` relies on: two 300x200 windows cascade to
/// frames at 48,52 and 80,84, Left's title bar at 150,60 is clear of Right, and
/// after a 200 px drag Left's close control covers 533,69.
#[test]
fn the_desk_windows_case_coordinates_hold() {
    use crate::chrome::{self, Part};
    use crate::desk::{Button, Hit, Input};
    use crate::geom::{Point, Size};
    let mut desk = Desk::new(1280, 800);
    let size = Size { w: 300, h: 200 };
    let (left, _) = desk.open(1, "deskdemo", "Left", size, true);
    let (right, _) = desk.open(1, "deskdemo", "Right", size, true);
    assert_eq!(
        desk.window(left).unwrap().frame,
        Rect::new(48, 52, 302, 234)
    );
    assert_eq!(
        desk.window(right).unwrap().frame,
        Rect::new(80, 84, 302, 234)
    );
    assert_eq!(desk.pointer(), Point { x: 640, y: 400 });
    assert_eq!(
        desk.hit(Point { x: 150, y: 60 }),
        Hit::Window(left, Part::Title)
    );
    desk.input(Input::Motion(Point { x: 150, y: 60 }));
    desk.input(Input::Button {
        button: Button::Left,
        down: true,
    });
    desk.input(Input::Motion(Point { x: 350, y: 60 }));
    desk.input(Input::Button {
        button: Button::Left,
        down: false,
    });
    let frame = desk.window(left).unwrap().frame;
    assert_eq!(frame, Rect::new(248, 52, 302, 234));
    assert_eq!(desk.focus(), Focus::Window(left));
    let (_, close) = chrome::controls(frame)[0];
    assert!(close.contains(Point { x: 533, y: 69 }), "{close:?}");
    assert_eq!(
        desk.hit(Point { x: 533, y: 69 }),
        Hit::Window(left, Part::Close)
    );
}
