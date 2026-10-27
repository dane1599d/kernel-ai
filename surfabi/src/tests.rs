use crate::header::{self, Event, Front, Latched};
use crate::input::{InputEvent, Kind};
use crate::proto::{Event as Ev, GLOBALS, Interface, Objects, REGISTRY, Request};
use crate::tree::{Error as TreeError, Node, Role, Tree, Update, action, flag};
use crate::wire::{self, Error, Fixed, Reader, Writer};
use crate::{Format, MAX_MESSAGE, Rect};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

#[test]
fn a_message_round_trips_and_its_header_is_waylands() {
    let bytes = Writer::new(7, 3)
        .uint(5)
        .int(-2)
        .fixed(Fixed::from_int(-3))
        .string("héllo")
        .array(&[1, 2, 3])
        .finish()
        .unwrap();
    // Object, then size << 16 | opcode, little-endian.
    assert_eq!(&bytes[0..4], &7u32.to_le_bytes());
    assert_eq!(
        &bytes[4..8],
        &((bytes.len() as u32) << 16 | 3).to_le_bytes()
    );
    let (header, args, rest) = wire::split(&bytes).unwrap();
    assert_eq!(
        (header.object, header.opcode, usize::from(header.size)),
        (7, 3, bytes.len())
    );
    assert!(rest.is_empty());
    let mut r = Reader::new(args);
    assert_eq!(r.uint(), Ok(5));
    assert_eq!(r.int(), Ok(-2));
    assert_eq!(r.fixed().map(Fixed::trunc), Ok(-3));
    assert_eq!(r.string(), Ok("héllo"));
    assert_eq!(r.array(), Ok(&[1u8, 2, 3][..]));
    assert_eq!(r.end(), Ok(()));
}

#[test]
fn bad_sizes_strings_and_leftovers_are_refused() {
    let good = Writer::new(1, 0).string("ab").finish().unwrap();
    assert_eq!(wire::split(&good[..7]), Err(Error::Short));
    assert_eq!(wire::split(&good[..good.len() - 1]), Err(Error::Short));
    for size in [0u32, 4, 9, MAX_MESSAGE as u32 + 4] {
        let mut bad = good.clone();
        bad[4..8].copy_from_slice(&(size << 16).to_le_bytes());
        assert_eq!(wire::split(&bad).map(|_| ()), Err(Error::BadSize), "{size}");
    }
    // A string with no NUL, a NUL inside, null, and bad UTF-8.
    for raw in [
        &b"\x02\0\0\0ab\0\0"[..],
        b"\x03\0\0\0a\0b\0",
        b"\0\0\0\0",
        b"\x02\0\0\0\xff\0\0\0",
    ] {
        assert_eq!(Reader::new(raw).string(), Err(Error::BadString), "{raw:?}");
    }
    assert_eq!(
        Reader::new(b"\xff\xff\xff\xff").array(),
        Err(Error::Truncated)
    );
    assert_eq!(Reader::new(&[0; 8]).end(), Err(Error::Trailing));
    assert_eq!(Reader::new(&[0; 4]).object(), Err(Error::BadArg));
    assert_eq!(
        Reader::new(&(-1i32).to_le_bytes()).size(),
        Err(Error::BadArg)
    );
    assert!(Writer::new(1, 0).array(&[0; MAX_MESSAGE]).finish().is_err());
}

/// A client's first window: bind a pool and a surface, make a buffer, give the
/// surface a toplevel and a tree, attach, damage, commit.
fn first_window() -> Vec<(u32, Request<'static>)> {
    vec![
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
            2,
            Request::PoolCreate {
                size: 4096 + 64 * 64 * 4,
            },
        ),
        (
            2,
            Request::PoolBuffer {
                id: 4,
                offset: 4096,
                width: 64,
                height: 64,
                stride: 256,
                format: Format::Xrgb8888,
            },
        ),
        (3, Request::GetToplevel { id: 5 }),
        (5, Request::SetTitle("Notes")),
        (3, Request::GetA11y { id: 6 }),
        (3, Request::Attach { buffer: 4 }),
        (
            3,
            Request::DamageBuffer(Rect {
                x: 0,
                y: 0,
                w: 64,
                h: 64,
            }),
        ),
        (3, Request::Commit),
    ]
}

#[test]
fn requests_round_trip_and_build_the_object_table() {
    let mut objects = Objects::new();
    for (object, request) in first_window() {
        let bytes = request.encode(object).unwrap();
        let (header, args, rest) = wire::split(&bytes).unwrap();
        assert!(rest.is_empty());
        let (_, got) = objects.receive(header.object, header.opcode, args).unwrap();
        assert_eq!(got, request);
    }
    assert_eq!(objects.interface(4), Some(Interface::Buffer));
    assert_eq!(objects.interface(5), Some(Interface::Toplevel));
    assert_eq!(objects.len(), 6);
    let destroy = Request::BufferDestroy.encode(4).unwrap();
    let (h, args, _) = wire::split(&destroy).unwrap();
    objects.receive(h.object, h.opcode, args).unwrap();
    assert_eq!(objects.interface(4), None);
}

#[test]
fn the_table_refuses_what_names_nothing_or_takes_a_live_id() {
    let mut objects = Objects::new();
    let send = |o: &mut Objects, object, request: Request| {
        let bytes = request.encode(object).unwrap();
        let (h, args, _) = wire::split(&bytes).unwrap();
        o.receive(h.object, h.opcode, args).map(|_| ())
    };
    // A global by the wrong name or number, a version past 1, a taken id.
    assert_eq!(
        send(
            &mut objects,
            REGISTRY,
            Request::Bind {
                name: 1,
                interface: "seat",
                version: 1,
                id: 2
            }
        ),
        Err(Error::BadArg)
    );
    assert_eq!(
        send(
            &mut objects,
            REGISTRY,
            Request::Bind {
                name: 9,
                interface: "pool",
                version: 1,
                id: 2
            }
        ),
        Err(Error::BadArg)
    );
    assert_eq!(
        send(
            &mut objects,
            REGISTRY,
            Request::Bind {
                name: 1,
                interface: "pool",
                version: 2,
                id: 2
            }
        ),
        Err(Error::BadArg)
    );
    assert_eq!(
        send(
            &mut objects,
            REGISTRY,
            Request::Bind {
                name: 1,
                interface: "pool",
                version: 1,
                id: 1
            }
        ),
        Err(Error::BadArg)
    );
    assert_eq!(
        send(&mut objects, 9, Request::Commit),
        Err(Error::UnknownObject)
    );
    send(
        &mut objects,
        REGISTRY,
        Request::Bind {
            name: 2,
            interface: "surface",
            version: 1,
            id: 3,
        },
    )
    .unwrap();
    // Attach something that is not a buffer; a popup on something that is not a surface.
    assert_eq!(
        send(&mut objects, 3, Request::Attach { buffer: 3 }),
        Err(Error::BadArg)
    );
    assert_eq!(
        send(
            &mut objects,
            3,
            Request::GetPopup {
                id: 4,
                parent: REGISTRY
            }
        ),
        Err(Error::BadArg)
    );
    send(&mut objects, 3, Request::GetPopup { id: 4, parent: 3 }).unwrap();
    assert_eq!(objects.interface(4), Some(Interface::Popup));
    // An opcode the interface lacks (a surface's commit sent to the registry),
    // and a bind with no arguments.
    assert_eq!(
        objects.receive(3, 99, &[]).map(|_| ()),
        Err(Error::UnknownOpcode)
    );
    assert_eq!(
        objects.receive(REGISTRY, 4, &[]).map(|_| ()),
        Err(Error::UnknownOpcode)
    );
    assert_eq!(
        objects.receive(REGISTRY, 0, &[]).map(|_| ()),
        Err(Error::Truncated)
    );
}

#[test]
fn events_round_trip() {
    let states = 4u32.to_le_bytes();
    let events = [
        (
            Interface::Registry,
            Ev::Global {
                name: 1,
                interface: GLOBALS[0].name(),
                version: 1,
            },
        ),
        (Interface::Buffer, Ev::Release),
        (Interface::Surface, Ev::PreferredScale(180)),
        (
            Interface::Toplevel,
            Ev::Configure {
                serial: 9,
                width: 800,
                height: 600,
                states: &states,
            },
        ),
        (Interface::Toplevel, Ev::Close),
        (
            Interface::Popup,
            Ev::PopupConfigure {
                x: -4,
                y: 10,
                width: 120,
                height: 80,
            },
        ),
        (Interface::Seat, Ev::FocusIn { surface: 3 }),
        (Interface::Seat, Ev::Modifiers(0b101)),
        (
            Interface::A11y,
            Ev::ActionRequest {
                node: 7,
                action: 2,
                value: "milk and eggs",
            },
        ),
        (
            Interface::Panel,
            Ev::PanelConfigure {
                width: 1280,
                height: 32,
            },
        ),
    ];
    for (interface, event) in events {
        let bytes = event.encode(5).unwrap();
        let (h, args, _) = wire::split(&bytes).unwrap();
        assert_eq!(Ev::decode(interface, h.opcode, args), Ok(event));
    }
    // A states array that is not whole u32s.
    let bad = Ev::Configure {
        serial: 1,
        width: 1,
        height: 1,
        states: &[1, 2],
    }
    .encode(5)
    .unwrap();
    let (h, args, _) = wire::split(&bad).unwrap();
    assert_eq!(
        Ev::decode(Interface::Toplevel, h.opcode, args),
        Err(Error::BadArg)
    );
}

const POOL: u32 = 4096 + 100 * 50 * 4;

fn front() -> Front {
    Front {
        offset: 4096,
        width: 100,
        height: 50,
        stride: 400,
        format: Format::Argb8888,
    }
}

#[test]
fn a_commit_latches_once_with_its_damage_cut_to_the_buffer() {
    let mut page = header::new_page(POOL);
    assert_eq!(header::latch(&page, POOL), Ok(None));
    let damage = [
        Rect {
            x: 10,
            y: 10,
            w: 20,
            h: 5,
        },
        Rect {
            x: 90,
            y: 40,
            w: 50,
            h: 50,
        },
        Rect {
            x: 200,
            y: 0,
            w: 5,
            h: 5,
        },
    ];
    header::commit(&mut page, front(), &damage);
    let latched = header::latch(&page, POOL).unwrap().unwrap();
    assert_eq!(
        latched,
        Latched {
            seq: 1,
            front: front(),
            damage: vec![
                Rect {
                    x: 10,
                    y: 10,
                    w: 20,
                    h: 5
                },
                Rect {
                    x: 90,
                    y: 40,
                    w: 10,
                    h: 10
                }
            ],
        }
    );
    header::set_latched(&mut page, latched.seq);
    assert_eq!(header::latch(&page, POOL), Ok(None));
    // Past 32 rectangles, their bounding box.
    let many: Vec<Rect> = (0..40)
        .map(|n| Rect {
            x: n,
            y: n,
            w: 1,
            h: 1,
        })
        .collect();
    header::commit(&mut page, front(), &many);
    let latched = header::latch(&page, POOL).unwrap().unwrap();
    assert_eq!(
        latched.damage,
        vec![Rect {
            x: 0,
            y: 0,
            w: 40,
            h: 40
        }]
    );
}

#[test]
fn a_page_the_client_wrote_badly_is_refused() {
    let page = |f: Front, pool: u32| {
        let mut p = header::new_page(pool);
        header::commit(&mut p, f, &[]);
        header::latch(&p, POOL)
    };
    assert_eq!(page(front(), POOL + 4), Err(header::Error::OutOfPool));
    assert_eq!(
        page(
            Front {
                offset: 0,
                ..front()
            },
            POOL
        ),
        Err(header::Error::OutOfPool)
    );
    assert_eq!(
        page(
            Front {
                height: 51,
                ..front()
            },
            POOL
        ),
        Err(header::Error::OutOfPool)
    );
    assert_eq!(
        page(
            Front {
                stride: 399,
                ..front()
            },
            POOL
        ),
        Err(header::Error::BadStride)
    );
    assert_eq!(
        page(
            Front {
                width: u32::MAX,
                ..front()
            },
            POOL
        ),
        Err(header::Error::BadStride)
    );
    let mut bad = header::new_page(POOL);
    header::commit(&mut bad, front(), &[]);
    bad[16] = 9;
    assert_eq!(header::latch(&bad, POOL), Err(header::Error::BadFormat));
    bad[0] = b'X';
    assert_eq!(header::latch(&bad, POOL), Err(header::Error::BadMagic));
}

fn key(code: u16, text: &str) -> Event {
    let mut bytes = [0u8; 4];
    bytes[..text.len()].copy_from_slice(text.as_bytes());
    Event::Input {
        event: InputEvent {
            time_ns: 42,
            kind: Kind::Key,
            code,
            value: 1,
        },
        x: Fixed::from_int(3).0,
        y: Fixed::from_int(4).0,
        text: bytes,
        len: text.len() as u8,
    }
}

#[test]
fn events_go_round_the_ring_and_a_full_ring_drops() {
    let mut page = header::new_page(POOL);
    assert_eq!(header::take(&mut page), Ok(None));
    let events = [
        Event::FrameDone { seq: 1, time_ns: 7 },
        Event::BufferRelease { offset: 4096 },
        key(0x04, "é"),
    ];
    for _ in 0..30 {
        for event in &events {
            assert!(header::post(&mut page, event));
        }
        for event in &events {
            assert_eq!(header::take(&mut page), Ok(Some(*event)));
        }
    }
    for n in 0..header::RING as u32 {
        assert!(header::post(
            &mut page,
            &Event::FrameDone { seq: n, time_ns: 0 }
        ));
    }
    assert!(!header::post(&mut page, &events[0]));
    assert_eq!(header::dropped(&page), 1);
    assert_eq!(
        header::take(&mut page),
        Ok(Some(Event::FrameDone { seq: 0, time_ns: 0 }))
    );
}

#[test]
fn input_events_are_sixteen_bytes() {
    let event = InputEvent {
        time_ns: 1 << 40,
        kind: Kind::Wheel,
        code: 1,
        value: -3,
    };
    assert_eq!(InputEvent::decode(&event.encode()), Some(event));
    let mut bad = event.encode();
    bad[8] = 9;
    assert_eq!(InputEvent::decode(&bad), None);
    let mut bad = event.encode();
    bad[9] = 1;
    assert_eq!(InputEvent::decode(&bad), None);
}

pub(crate) fn node(id: u32, role: Role, name: &str, children: &[u32]) -> Node {
    Node {
        id,
        role,
        flags: flag::FOCUSABLE,
        actions: action::CLICK | action::FOCUS,
        x: 1,
        y: -2,
        w: 30,
        h: 20,
        name: String::from(name),
        value: String::new(),
        children: children.to_vec(),
    }
}

pub(crate) fn notes_update() -> Update {
    Update {
        root: 1,
        focus: 3,
        nodes: vec![
            node(1, Role::Window, "Notes", &[2, 3, 4]),
            node(2, Role::Button, "New note", &[]),
            Node {
                value: String::from("Groceries"),
                ..node(3, Role::TextInput, "Title", &[])
            },
            node(4, Role::List, "Notes", &[]),
        ],
    }
}

#[test]
fn a_tree_update_round_trips_and_applies() {
    let update = notes_update();
    let bytes = update.encode();
    assert_eq!(Update::decode(&bytes), Ok(update.clone()));
    let mut tree = Tree::default();
    tree.apply(update).unwrap();
    assert_eq!(tree.node(3).map(|n| n.value.as_str()), Some("Groceries"));
    // A later update changes one node and drops another from the root's children.
    let change = Update {
        root: 1,
        focus: 2,
        nodes: vec![node(1, Role::Window, "Notes", &[2, 3])],
    };
    tree.apply(change).unwrap();
    assert_eq!(tree.node(4), None);
    assert_eq!(tree.nodes.len(), 3);
}

#[test]
fn a_bad_tree_leaves_the_old_one() {
    let mut tree = Tree::default();
    tree.apply(notes_update()).unwrap();
    let before = tree.clone();
    let cases = [
        (
            Update {
                root: 9,
                focus: 1,
                nodes: vec![],
            },
            TreeError::Dangling,
        ),
        (
            Update {
                root: 1,
                focus: 9,
                nodes: vec![],
            },
            TreeError::Dangling,
        ),
        (
            Update {
                root: 1,
                focus: 1,
                nodes: vec![node(2, Role::Button, "b", &[1])],
            },
            TreeError::Cycle,
        ),
        (
            Update {
                root: 1,
                focus: 1,
                nodes: vec![node(2, Role::Button, "b", &[7])],
            },
            TreeError::Dangling,
        ),
        (
            Update {
                root: 1,
                focus: 1,
                nodes: vec![node(4, Role::List, "l", &[2])],
            },
            TreeError::Cycle,
        ),
    ];
    for (update, want) in cases {
        assert_eq!(tree.apply(update), Err(want));
        assert_eq!(tree, before);
    }
    // Bytes that are not a tree.
    let mut bytes = notes_update().encode();
    bytes[12 + 4] = 99;
    assert_eq!(Update::decode(&bytes), Err(TreeError::BadRole));
    let mut bytes = notes_update().encode();
    bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(Update::decode(&bytes), Err(TreeError::Truncated));
    let twice = Update {
        root: 1,
        focus: 1,
        nodes: vec![
            node(1, Role::Window, "a", &[]),
            node(1, Role::Window, "b", &[]),
        ],
    };
    assert_eq!(Update::decode(&twice.encode()), Err(TreeError::BadId));
    let control = Update {
        root: 1,
        focus: 1,
        nodes: vec![node(1, Role::Window, "a\u{7}", &[])],
    };
    assert_eq!(Update::decode(&control.encode()), Err(TreeError::BadText));
}

#[test]
fn a_tree_stays_within_its_node_cap() {
    let n = crate::tree::MAX_NODES as u32;
    let children: Vec<u32> = (2..=n + 1).collect();
    let mut nodes = vec![node(1, Role::Window, "w", &children)];
    nodes.extend((2..=n + 1).map(|id| node(id, Role::Label, "", &[])));
    let mut tree = Tree::default();
    assert_eq!(
        tree.apply(Update {
            root: 1,
            focus: 1,
            nodes
        }),
        Err(TreeError::TooBig)
    );
}
