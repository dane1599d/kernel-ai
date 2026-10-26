//! Hostile objects: mutated and truncated wcc objects never panic the linker, and
//! whatever it accepts it writes as a well-formed module.

mod common;

#[path = "../../testkit/hostile.rs"]
mod hostile;

use common::{FIXTURES, allow, fixture};
use kailink::{Layout, Session};

fn well_formed(wasm: &[u8]) {
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        payload.expect("kailink wrote a malformed module");
    }
}

/// A session where every fixture's undefined names are defined.
fn rich_session() -> Session {
    let mut session = Session::new(Layout::default());
    for name in ["define_sq", "global", "fib", "redefine0", "redefine1"] {
        let linked = session.link(&fixture(name), &allow).unwrap();
        session.commit(&linked);
    }
    session
}

#[test]
fn mutated_objects_never_panic() {
    let session = rich_session();
    for (i, name) in FIXTURES.iter().enumerate() {
        let sample = fixture(name);
        hostile::fuzz(&sample, 0x4b41 + i as u64, 1500, |bytes| {
            if let Ok(linked) = session.link(bytes, &allow) {
                well_formed(&linked.wasm);
            }
        });
    }
}

#[test]
fn truncated_objects_never_panic() {
    let session = rich_session();
    for name in FIXTURES {
        hostile::truncations(&fixture(name), |_, bytes| {
            if let Ok(linked) = session.link(bytes, &allow) {
                well_formed(&linked.wasm);
            }
        });
    }
}

#[test]
fn every_fixture_links_to_a_well_formed_module() {
    let session = rich_session();
    for name in FIXTURES {
        if let Ok(linked) = session.link(&fixture(name), &allow) {
            well_formed(&linked.wasm);
        }
    }
}
