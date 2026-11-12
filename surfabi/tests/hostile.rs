//! Hostile input to every parser `desktop` runs on an app's bytes: the message
//! stream, the header page and the accessibility tree, cut at every byte, with
//! every field set to extreme values and mutated thousands of times. Each
//! either parses or fails with an error: none panics, none loops, and none
//! allocates more than a small multiple of its input.

#[path = "../../testkit/hostile.rs"]
mod hostile;

#[path = "../../fuzz/surfabi_harness.rs"]
mod harness;

use surfabi::proto::{REGISTRY, Request};
use surfabi::tree::{Node, Role, Tree, Update};
use surfabi::wire::{self, Error};

const ALLOC_PER_BYTE: usize = 16;
const ALLOC_FLOOR: usize = 64 * 1024;

fn check_stream(bytes: &[u8]) {
    let (_, peak) = hostile::peak_alloc(|| harness::serve(bytes));
    assert!(
        peak <= ALLOC_PER_BYTE * bytes.len() + ALLOC_FLOOR,
        "{peak} bytes for {}",
        bytes.len()
    );
}

#[test]
fn the_stream_at_every_prefix() {
    let sample = harness::stream();
    assert_eq!(harness::serve(&sample), Ok(16));
    hostile::truncations(&sample, |_, prefix| {
        // A prefix is a run of whole messages and then, at most, a short one.
        match harness::serve(prefix) {
            Ok(_) | Err(Error::Short) => {}
            Err(error) => panic!("{error:?} for a prefix of {} bytes", prefix.len()),
        }
    });
}

#[test]
fn the_stream_with_every_field_and_random_mutations() {
    let sample = harness::stream();
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
    assert_eq!(harness::serve(&bytes), Err(Error::BadArg));
}

fn check_page(bytes: &[u8]) {
    harness::header_page(bytes);
}

#[test]
fn the_header_page_with_every_field_and_random_mutations() {
    let sample = harness::page();
    hostile::sweep_fields(&sample, 0..400, |_, mutated| check_page(mutated));
    hostile::sweep_fields(&sample, 2048..2240, |_, mutated| check_page(mutated));
    hostile::fuzz(&sample, 77, 4000, check_page);
}

fn check_tree(bytes: &[u8]) {
    let (_, peak) = hostile::peak_alloc(|| harness::tree_records(bytes));
    assert!(
        peak <= ALLOC_PER_BYTE * bytes.len() + ALLOC_FLOOR,
        "{peak} bytes for {}",
        bytes.len()
    );
}

#[test]
fn the_tree_at_every_prefix_field_and_mutation() {
    let sample = harness::update().encode();
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

/// Writes each fuzz target's seeds, the samples above, under `FUZZ_SEED_DIR`
/// (`.github/workflows/fuzz.yml` runs it before libFuzzer).
#[test]
#[ignore = "writes the fuzz corpus seeds; fuzz.yml runs it"]
fn write_fuzz_seeds() {
    let dir = std::env::var("FUZZ_SEED_DIR").expect("FUZZ_SEED_DIR names the corpus folder");
    for (target, bytes) in harness::seeds() {
        let folder = std::path::Path::new(&dir).join(target);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("seed-0"), bytes).unwrap();
    }
}
