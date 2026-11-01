//! Hostile input to every parser `desktop` runs on an app's bytes: the message
//! stream, the header page and the accessibility tree, cut at every byte, with
//! every field set to extreme values and mutated thousands of times. Each
//! either parses or fails with an error: none panics, none loops, and none
//! allocates more than a small multiple of its input.

#[path = "../../testkit/hostile.rs"]
mod hostile;

use surfabi::header::{self, Event, Front};
use surfabi::input::{InputEvent, Kind};
use surfabi::proto::{Objects, REGISTRY, Request};
use surfabi::tree::{Node, Role, Tree, Update, action, flag};
use surfabi::wire::{self, Error};
use surfabi::{Format, Rect};

const ALLOC_PER_BYTE: usize = 16;
const ALLOC_FLOOR: usize = 64 * 1024;

/// A client's first frames, as one byte stream.
fn stream() -> Vec<u8> {
    let requests = [
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
        (5, Request::SetAppId("app:notes")),
        (3, Request::GetA11y { id: 6 }),
        (6, Request::TreeUpdate(&[1, 2, 3, 4, 5])),
        (
            6,
            Request::BindAction {
                node: 2,
                action: "notes_save",
            },
        ),
        (5, Request::SetUnsaved(true)),
        (5, Request::SetAgentPrivate(true)),
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
        (4, Request::BufferDestroy),
    ];
    requests
        .iter()
        .flat_map(|(object, request)| request.encode(*object).unwrap())
        .collect()
}

/// What `desktop` does with a client's bytes: split off each message, decode
/// it against the connection's objects, stop at the first error.
fn serve(bytes: &[u8]) -> Result<usize, Error> {
    let mut objects = Objects::new();
    let mut rest = bytes;
    let mut handled = 0;
    while !rest.is_empty() {
        let (header, args, next) = wire::split(rest)?;
        objects.receive(header.object, header.opcode, args)?;
        assert!(objects.len() <= surfabi::proto::MAX_OBJECTS);
        rest = next;
        handled += 1;
    }
    Ok(handled)
}

fn check_stream(bytes: &[u8]) {
    let (_, peak) = hostile::peak_alloc(|| serve(bytes));
    assert!(
        peak <= ALLOC_PER_BYTE * bytes.len() + ALLOC_FLOOR,
        "{peak} bytes for {}",
        bytes.len()
    );
}

#[test]
fn the_stream_at_every_prefix() {
    let sample = stream();
    assert_eq!(serve(&sample), Ok(16));
    hostile::truncations(&sample, |_, prefix| {
        // A prefix is a run of whole messages and then, at most, a short one.
        match serve(prefix) {
            Ok(_) | Err(Error::Short) => {}
            Err(error) => panic!("{error:?} for a prefix of {} bytes", prefix.len()),
        }
    });
}

#[test]
fn the_stream_with_every_field_and_random_mutations() {
    let sample = stream();
    hostile::sweep_fields(&sample, 0..sample.len(), |_, mutated| check_stream(mutated));
    for seed in [5, 0x2545_f491_4f6c_dd1d] {
        hostile::fuzz(&sample, seed, 6000, check_stream);
    }
}

#[test]
fn a_flood_of_objects_stops_at_the_cap() {
    let mut bytes = Vec::new();
    for id in 2..3000 {
        let bind = Request::Bind {
            name: 2,
            interface: "surface",
            version: 1,
            id,
        };
        bytes.extend(bind.encode(REGISTRY).unwrap());
    }
    assert_eq!(serve(&bytes), Err(Error::BadArg));
}

const POOL: u32 = 4096 + 32 * 32 * 4;

fn page() -> [u8; header::PAGE] {
    let mut page = header::new_page(POOL);
    let front = Front {
        offset: 4096,
        width: 32,
        height: 32,
        stride: 128,
        format: Format::Argb8888,
    };
    header::commit(
        &mut page,
        front,
        &[Rect {
            x: 1,
            y: 2,
            w: 3,
            h: 4,
        }; 3],
    );
    let key = InputEvent {
        time_ns: 9,
        kind: Kind::Key,
        code: 4,
        value: 1,
    };
    for _ in 0..5 {
        header::post(
            &mut page,
            &Event::Input {
                event: key,
                x: 256,
                y: 512,
                text: *b"a\0\0\0",
                len: 1,
            },
        );
    }
    page
}

fn check_page(bytes: &[u8]) {
    let Ok(page) = <[u8; header::PAGE]>::try_from(bytes) else {
        return;
    };
    let mut page = page;
    if let Ok(Some(latched)) = header::latch(&page, POOL) {
        assert!(latched.damage.len() <= header::MAX_DAMAGE);
        let end =
            latched.front.offset as u64 + latched.front.stride as u64 * latched.front.height as u64;
        assert!(end <= u64::from(POOL));
        for r in latched.damage {
            assert!(r.x + r.w <= latched.front.width && r.y + r.h <= latched.front.height);
        }
    }
    // The client's side of the ring ends, whatever `desktop` wrote.
    for _ in 0..=header::RING {
        if header::take(&mut page) == Ok(None) {
            break;
        }
    }
    header::post(&mut page, &Event::BufferRelease { offset: 4096 });
}

#[test]
fn the_header_page_with_every_field_and_random_mutations() {
    let sample = page();
    hostile::sweep_fields(&sample, 0..400, |_, mutated| check_page(mutated));
    hostile::sweep_fields(&sample, 2048..2240, |_, mutated| check_page(mutated));
    hostile::fuzz(&sample, 77, 4000, check_page);
}

fn update() -> Update {
    let node = |id, role, name: &str, children: Vec<u32>| Node {
        id,
        role,
        flags: flag::FOCUSABLE,
        actions: action::CLICK,
        x: 0,
        y: 0,
        w: 10,
        h: 10,
        name: name.into(),
        value: "v".into(),
        children,
    };
    Update {
        root: 1,
        focus: 2,
        nodes: vec![
            node(1, Role::Window, "Notes", vec![2, 3]),
            node(2, Role::Button, "New note", vec![]),
            node(3, Role::List, "Notes", vec![4]),
            node(4, Role::ListItem, "Groceries", vec![]),
        ],
    }
}

fn check_tree(bytes: &[u8]) {
    let (_, peak) = hostile::peak_alloc(|| {
        let mut tree = Tree::default();
        if let Ok(update) = Update::decode(bytes) {
            let _ = tree.apply(update);
        }
        tree.nodes.len()
    });
    assert!(
        peak <= ALLOC_PER_BYTE * bytes.len() + ALLOC_FLOOR,
        "{peak} bytes for {}",
        bytes.len()
    );
}

#[test]
fn the_tree_at_every_prefix_field_and_mutation() {
    let sample = update().encode();
    let mut tree = Tree::default();
    tree.apply(Update::decode(&sample).unwrap()).unwrap();
    hostile::truncations(&sample, |n, prefix| {
        if n < sample.len() {
            assert!(Update::decode(prefix).is_err());
        }
    });
    hostile::sweep_fields(&sample, 0..sample.len(), |_, mutated| check_tree(mutated));
    for seed in [3, 0x9e37_79b9_7f4a_7c15] {
        hostile::fuzz(&sample, seed, 6000, check_tree);
    }
}

#[test]
fn a_deep_chain_and_a_wide_tree_cost_no_recursion() {
    let n = 4000;
    let node = |id: u32, children: Vec<u32>| Node {
        id,
        role: Role::Group,
        flags: 0,
        actions: 0,
        x: 0,
        y: 0,
        w: 0,
        h: 0,
        name: String::new(),
        value: String::new(),
        children,
    };
    let chain: Vec<Node> = (1..=n)
        .map(|id| node(id, if id < n { vec![id + 1] } else { vec![] }))
        .collect();
    let mut tree = Tree::default();
    tree.apply(Update {
        root: 1,
        focus: n,
        nodes: chain,
    })
    .unwrap();
    assert_eq!(tree.nodes.len(), n as usize);
}

/// The theme rides the registry as opcode 1, so a client that takes only
/// opcode 0 (`Global`) as a greeting never mistakes it for a new desktop.
#[test]
fn the_theme_event_round_trips_and_is_no_greeting() {
    use surfabi::proto::{Event as Wire, Interface};
    let bytes = Wire::Theme("light").encode(REGISTRY).unwrap();
    let (header, args, rest) = wire::split(&bytes).unwrap();
    assert!(rest.is_empty());
    assert_eq!((header.object, header.opcode), (REGISTRY, 1));
    assert_eq!(
        Wire::decode(Interface::Registry, header.opcode, args),
        Ok(Wire::Theme("light"))
    );
    let global = Wire::Global {
        name: 1,
        interface: "pool",
        version: 1,
    }
    .encode(REGISTRY)
    .unwrap();
    assert_eq!(wire::split(&global).unwrap().0.opcode, 0);
}
