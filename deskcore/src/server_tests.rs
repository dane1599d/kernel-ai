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

/// Keys, clicks and moves for a client's window go to its pool's header ring,
/// a key with the text it types; the seat hears the modifiers.
#[test]
fn input_for_a_window_goes_to_its_pool_with_its_text() {
    use crate::desk::Button;
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
    events(&mut client);
    let key = |usage, down| Effect::Key {
        window,
        usage,
        down,
    };
    client.apply(&[key(0xE1, true), key(0x04, true), key(0x04, false)]);
    let posts = client.take_posts();
    let typed: Vec<(u32, Kind, u16, i32, &[u8])> = posts
        .iter()
        .map(|(pool, e)| match e {
            Ring::Input {
                event, text, len, ..
            } => (
                *pool,
                event.kind,
                event.code,
                event.value,
                &text[..usize::from(*len)],
            ),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        typed,
        [
            (2, Kind::Key, 0xE1, 1, &b""[..]),
            (2, Kind::Key, 0x04, 1, &b"A"[..]),
            (2, Kind::Key, 0x04, 0, &b""[..]),
        ]
    );
    // Each key says what was held with it, in the ring: Shift's own event too.
    let held: Vec<i32> = posts
        .iter()
        .map(|(_, e)| match e {
            Ring::Input { x, .. } => *x,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(held, [1 << 8, 1 << 8, 1 << 8]);
    // The seat (7) heard Shift.
    let sent = events(&mut client);
    let modifiers = Event::decode(Interface::Seat, sent[0].1, &sent[0].2);
    assert_eq!((sent[0].0, modifiers), (7, Ok(Event::Modifiers(1))));
    // Ctrl held: the key comes with no text.
    client.apply(&[key(0xE1, false), key(0xE0, true), key(0x06, true)]);
    let ctrl_c = client.take_posts();
    assert!(matches!(ctrl_c[2].1, Ring::Input { len: 0, .. }));
    let held: Vec<i32> = ctrl_c
        .iter()
        .map(|(_, e)| match e {
            Ring::Input { x, .. } => *x,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        held,
        [0, 2 << 8, 2 << 8],
        "Shift let go, Ctrl down, C with Ctrl"
    );
    // A click carries its place in 24.8; another window's input is not ours.
    let at = Point { x: 10, y: 20 };
    client.apply(&[Effect::Button {
        window,
        at,
        button: Button::Left,
        down: true,
    }]);
    let click = client.take_posts();
    assert!(matches!(
        click[..],
        [(
            2,
            Ring::Input {
                x: 2560,
                y: 5120,
                ..
            }
        )]
    ));
    client.apply(&[Effect::Key {
        window: crate::desk::WindowId(window.0 + 100),
        usage: 0x04,
        down: true,
    }]);
    assert!(client.take_posts().is_empty());
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
    // Test pixels, not colours: built so look's lint does not take them for one.
    let argb = |a: u32, rgb: u32| (a << 24) | rgb;
    px.fill(
        Rect::new(-2, -2, 4, 4),
        Rect::new(0, 0, 8, 4),
        argb(0xFF, 0x11_2233),
    );
    assert_eq!(
        &px.data[..3],
        &[argb(0xFF, 0x11_2233), argb(0xFF, 0x11_2233), 0]
    );
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
    assert_eq!(
        over(argb(0xFF, 0x12_3456), argb(0xFF, 0)),
        argb(0xFF, 0x12_3456)
    );
    assert_eq!(over(0, argb(0xFF, 0x10_2030)), argb(0xFF, 0x10_2030));
    // Half-transparent black over white: grey, opaque.
    assert_eq!(
        over(0x8000_0000, argb(0xFF, 0xFF_FFFF)),
        argb(0xFF, 0x7F_7F7F)
    );
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

/// A window with an `a11y` object (8) and its tree: a window, a button that
/// clicks, a text field.
fn tree_window() -> (Desk, Client, crate::desk::WindowId) {
    let mut desk = Desk::new(1280, 800);
    let mut client = Client::new(5, "notes");
    client.handle(&mut desk, &setup(), true).unwrap();
    client.pool_mapped(2);
    let needs = client.handle(&mut desk, &show(), false).unwrap();
    let [Need::Show { window, .. }] = needs[..] else {
        panic!("{needs:?}");
    };
    client
        .handle(&mut desk, &bytes(&[(3, Request::GetA11y { id: 8 })]), false)
        .unwrap();
    (desk, client, window)
}

fn tree_bytes(button_name: &str) -> Vec<u8> {
    use surfabi::tree::{Node, Role, Update, action};
    let node = |id, role, name: &str, actions, children: Vec<u32>| Node {
        id,
        role,
        flags: 0,
        actions,
        x: 0,
        y: 0,
        w: 10,
        h: 10,
        name: name.into(),
        value: alloc::string::String::new(),
        children,
    };
    Update {
        root: 1,
        focus: 1,
        nodes: vec![
            node(1, Role::Window, "Notes", 0, vec![2, 3]),
            node(2, Role::Button, button_name, action::CLICK, vec![]),
            node(3, Role::TextInput, "Title", action::SET_VALUE, vec![]),
        ],
    }
    .encode()
}

#[test]
fn a_committed_tree_is_kept_and_may_come_in_parts() {
    let (mut desk, mut client, window) = tree_window();
    assert_eq!(client.tree(window).unwrap().nodes.len(), 0);
    let tree = tree_bytes("New note");
    let (a, b) = tree.split_at(40);
    let parts = bytes(&[(8, Request::TreeUpdate(a)), (8, Request::TreeUpdate(b))]);
    client.handle(&mut desk, &parts, false).unwrap();
    // Nothing applies before the commit.
    assert_eq!(client.tree(window).unwrap().nodes.len(), 0);
    client
        .handle(&mut desk, &bytes(&[(8, Request::A11yCommit)]), false)
        .unwrap();
    let kept = client.tree(window).unwrap();
    assert_eq!(kept.node(2).unwrap().name, "New note");
    assert_eq!(kept.nodes.len(), 3);
}

#[test]
fn a_bad_tree_closes_the_connection_and_the_old_tree_stays() {
    let (mut desk, mut client, window) = tree_window();
    let good = bytes(&[
        (8, Request::TreeUpdate(&tree_bytes("New note"))),
        (8, Request::A11yCommit),
    ]);
    client.handle(&mut desk, &good, false).unwrap();
    let mut bad = tree_bytes("Evil");
    bad[8] = 0xFF; // a node count past the bytes
    let bad = bytes(&[(8, Request::TreeUpdate(&bad)), (8, Request::A11yCommit)]);
    assert_eq!(client.handle(&mut desk, &bad, false), Err(Error::BadArg));
    assert_eq!(
        client.tree(window).unwrap().node(2).unwrap().name,
        "New note"
    );
    // A tree for an `a11y` object nobody made is an unknown object.
    let stray = bytes(&[(9, Request::A11yCommit)]);
    assert!(client.handle(&mut desk, &stray, false).is_err());
}

#[test]
fn act_sends_an_action_request_only_for_an_action_the_node_offers() {
    let (mut desk, mut client, window) = tree_window();
    let tree = bytes(&[
        (8, Request::TreeUpdate(&tree_bytes("New note"))),
        (8, Request::A11yCommit),
    ]);
    client.handle(&mut desk, &tree, false).unwrap();
    events(&mut client);
    // CLICK is bit 0, SET_VALUE bit 2.
    assert!(client.act(window, 2, 0, ""));
    assert!(client.act(window, 3, 2, "Groceries"));
    assert!(!client.act(window, 2, 2, "x"), "a button takes no value");
    assert!(!client.act(window, 9, 0, ""), "no such node");
    assert!(!client.act(crate::desk::WindowId(99), 2, 0, ""));
    let sent = events(&mut client);
    let requests: Vec<Event> = sent
        .iter()
        .map(|(object, op, args)| {
            assert_eq!(*object, 8);
            Event::decode(Interface::A11y, *op, args).unwrap()
        })
        .collect();
    assert_eq!(
        requests,
        [
            Event::ActionRequest {
                node: 2,
                action: 0,
                value: ""
            },
            Event::ActionRequest {
                node: 3,
                action: 2,
                value: "Groceries"
            },
        ]
    );
}

#[test]
fn a_bound_node_names_its_action_and_unbinding_or_a_new_tree_drops_it() {
    let (mut desk, mut client, window) = tree_window();
    let tree = bytes(&[
        (8, Request::TreeUpdate(&tree_bytes("Save"))),
        (8, Request::A11yCommit),
        (
            8,
            Request::BindAction {
                node: 2,
                action: "notes_save",
            },
        ),
        (
            8,
            Request::BindAction {
                node: 9,
                action: "notes_delete",
            },
        ),
    ]);
    client.handle(&mut desk, &tree, false).unwrap();
    assert_eq!(client.bound_action(window, 2), Some("notes_save"));
    assert_eq!(client.bound_action(window, 3), None, "unbound: ask");
    assert_eq!(client.bound_action(window, 9), None, "not in the tree");
    let unbind = bytes(&[(
        8,
        Request::BindAction {
            node: 2,
            action: "",
        },
    )]);
    client.handle(&mut desk, &unbind, false).unwrap();
    assert_eq!(client.bound_action(window, 2), None);
}

#[test]
fn a_badly_named_or_one_too_many_binding_closes_the_connection() {
    // Only the shape: another app's row is the agent core's to refuse.
    for name in [
        "Notes_Save",
        "save",
        "notes save",
        "notes_",
        "_save",
        &"notes_".repeat(11),
    ] {
        let (mut desk, mut client, _) = tree_window();
        let bad = bytes(&[(
            8,
            Request::BindAction {
                node: 2,
                action: name,
            },
        )]);
        assert_eq!(
            client.handle(&mut desk, &bad, false),
            Err(Error::BadArg),
            "{name}"
        );
    }
    let (mut desk, mut client, _) = tree_window();
    let many: Vec<(u32, Request)> = (0..=crate::server::MAX_BOUND as u32)
        .map(|node| {
            (
                8,
                Request::BindAction {
                    node,
                    action: "notes_save",
                },
            )
        })
        .collect();
    assert_eq!(
        client.handle(&mut desk, &bytes(&many), false),
        Err(Error::BadArg)
    );
}

#[test]
fn a_window_is_saved_until_its_app_says_otherwise() {
    let (mut desk, mut client, window) = tree_window();
    assert!(!client.unsaved(window));
    client
        .handle(&mut desk, &bytes(&[(5, Request::SetUnsaved(true))]), false)
        .unwrap();
    assert!(client.unsaved(window));
    client
        .handle(&mut desk, &bytes(&[(5, Request::SetUnsaved(false))]), false)
        .unwrap();
    assert!(!client.unsaved(window));
}

#[test]
fn a_window_is_shared_with_the_agent_until_its_app_keeps_it_private() {
    let (mut desk, mut client, window) = tree_window();
    assert!(!client.agent_private(window));
    let private = |on| bytes(&[(5, Request::SetAgentPrivate(on))]);
    client.handle(&mut desk, &private(true), false).unwrap();
    assert!(client.agent_private(window));
    // Its unsaved changes are a separate flag.
    assert!(!client.unsaved(window));
    client.handle(&mut desk, &private(false), false).unwrap();
    assert!(!client.agent_private(window));
}

#[test]
fn a_window_of_another_name_binds_its_apps_rows() {
    // Notes' window runs as `notes-window` and binds the Notes app's `notes_add`.
    let mut desk = Desk::new(1280, 800);
    let mut client = Client::new(5, "notes-window");
    client.handle(&mut desk, &setup(), true).unwrap();
    client
        .handle(&mut desk, &bytes(&[(3, Request::GetA11y { id: 8 })]), false)
        .unwrap();
    let rows = bytes(&[(
        8,
        Request::BindAction {
            node: 2,
            action: "notes_add",
        },
    )]);
    assert_eq!(client.handle(&mut desk, &rows, false), Ok(vec![]));
}
