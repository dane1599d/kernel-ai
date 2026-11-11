//! The P3 fuzz targets' bodies for `surfabi`, the parsers `desktop` runs on
//! an app's bytes (plan p3-desktop sections 1 and 6: a fuzz target for every
//! new parser): the message stream, the header page and the accessibility
//! tree's records. Each `fuzz_targets/*.rs` calls one with libFuzzer's input,
//! and `surfabi/tests/hostile.rs` includes this file and runs each over hostile
//! mutations of the samples here, which also seed the targets' corpora. Each
//! must return for any input; only a bug in the code under it may panic.

#![allow(dead_code)]

use surfabi::header::{self, Event, Front};
use surfabi::input::{InputEvent, Kind};
use surfabi::proto::{Objects, REGISTRY, Request};
use surfabi::tree::{Node, Role, Tree, Update, action, flag};
use surfabi::wire::{self, Error};
use surfabi::{Format, Rect};

/// A client's first frames, as one byte stream.
pub fn stream() -> Vec<u8> {
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
pub fn serve(bytes: &[u8]) -> Result<usize, Error> {
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

/// The `surfabi_stream` target: a client's bytes, served until the first error.
pub fn surfabi_stream(bytes: &[u8]) {
    let _ = serve(bytes);
}

pub const POOL: u32 = 4096 + 32 * 32 * 4;

pub fn page() -> [u8; header::PAGE] {
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

/// The `header_page` target: a page as `desktop` latches it, then the
/// client's side of its ring. The input is the page's first bytes.
pub fn header_page(bytes: &[u8]) {
    // A page is always whole: the input fills it from the start, zeros after.
    let mut page = [0u8; header::PAGE];
    let n = bytes.len().min(header::PAGE);
    page[..n].copy_from_slice(&bytes[..n]);
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

/// A small window's tree, one node of each kind a window has.
pub fn update() -> Update {
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

/// The `tree_records` target: a tree update's records, decoded and applied.
/// Returns how many nodes the tree holds.
pub fn tree_records(bytes: &[u8]) -> usize {
    let mut tree = Tree::default();
    if let Ok(update) = Update::decode(bytes) {
        let _ = tree.apply(update);
    }
    tree.nodes.len()
}

/// The seeds of each target's corpus: the samples above.
pub fn seeds() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("surfabi_stream", stream()),
        ("header_page", page().to_vec()),
        ("tree_records", update().encode()),
    ]
}
