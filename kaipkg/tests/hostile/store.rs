//! The `kaipkg_store` fuzz target's body (plan P6 G41): what `pkg` reads from
//! the person's own disk, which `kaipkg/tests/hostile.rs` also runs over
//! hostile records. A later `fuzz/fuzz_targets/kaipkg_store.rs` includes this
//! file by `#[path]` and calls [`kaipkg_store`] with libFuzzer's input. It
//! must return for any input, and only a bug in kaipkg may panic.
//!
//! The input's first byte picks the reader; the rest is its input:
//!
//! - [`GENERATION`]: `Generation::parse`, a `kai-gen 1` file and its list of
//!   installed packages. One that parses writes back as the same text.
//! - [`TIME`]: `Time::parse`, a generation's time. One that parses formats
//!   back as the same text.
//! - [`CATALOG`]: `catalog::parse`, a `kai-catalog 1` answer. One that parses
//!   holds at most `MAX_ROWS` rows, each summary already as `summary_of`
//!   makes it, and writes back as the same text.
//! - [`STORE`]: the store as `pkg` reads it at start, `current`, `good` and
//!   the generation `current` names, from a volume holding the rest as all
//!   three files. A generation read is one `Generation::parse` takes.
//! - [`LOCAL_APP`]: `local::is_app`, the `kai-app 1` check of `pkg local`; the
//!   rest is a name's length, the name, then a Wasm artifact. An artifact it
//!   takes has a `kai.app` section of a `kai-app 1` line, then only `id` and
//!   `abi` lines, naming the name once.
//!
//! Text readers get the bytes as UTF-8, or nothing: `pkg` refuses a file
//! that is not text before it parses.

#![allow(dead_code)]

use kaipkg::catalog::{self, MAX_ROWS, summary_of};
use kaipkg::generation::{Generation, Time};
use kaipkg::local::{self, APP_SECTION};
use kaipkg::memvol::MemVolume;
use kaipkg::store::{self, Volume};

pub const GENERATION: u8 = 0;
pub const TIME: u8 = 1;
pub const CATALOG: u8 = 2;
pub const STORE: u8 = 3;
pub const LOCAL_APP: u8 = 4;
const READERS: u8 = 5;

pub fn kaipkg_store(data: &[u8]) {
    let Some((&which, rest)) = data.split_first() else {
        return;
    };
    match which % READERS {
        GENERATION => text(rest, generation),
        TIME => text(rest, time),
        CATALOG => text(rest, catalog),
        STORE => store(rest),
        _ => local_app(rest),
    }
}

fn text(bytes: &[u8], f: fn(&str)) {
    if let Ok(text) = core::str::from_utf8(bytes) {
        f(text);
    }
}

fn generation(text: &str) {
    if let Ok(g) = Generation::parse(text) {
        assert_eq!(
            g.format(),
            text,
            "a parsed generation writes back as itself"
        );
    }
}

fn time(text: &str) {
    if let Ok(t) = Time::parse(text) {
        assert_eq!(t.format(), text, "a parsed time formats back as itself");
    }
}

fn catalog(text: &str) {
    if let Ok(rows) = catalog::parse(text) {
        assert!(rows.len() <= MAX_ROWS);
        for row in &rows {
            assert_eq!(row.summary, summary_of(&row.summary), "{text:?}");
        }
        assert_eq!(
            catalog::format(&rows),
            text,
            "a parsed answer writes back as itself"
        );
    }
}

fn store(bytes: &[u8]) {
    let mut vol = MemVolume::default();
    for path in [store::CURRENT, store::GOOD] {
        vol.write(path, bytes).unwrap();
    }
    if let Ok(n) = store::current(&mut vol) {
        vol.write(&format!("{}/{n}", store::GENS), bytes).unwrap();
        if let Ok(g) = store::generation(&mut vol, n) {
            let text = core::str::from_utf8(bytes).expect("a generation read is text");
            assert_eq!(Generation::parse(text).as_ref(), Ok(&g));
        }
    }
    let _ = store::good(&mut vol);
    let _ = store::generations(&mut vol);
}

fn local_app(bytes: &[u8]) {
    let Some((&len, rest)) = bytes.split_first() else {
        return;
    };
    let len = usize::from(len).min(rest.len());
    let Ok(name) = core::str::from_utf8(&rest[..len]) else {
        return;
    };
    let artifact = &rest[len..];
    if local::is_app(artifact, name).is_err() {
        return;
    }
    let section = livecore::artifact::custom_section(artifact, APP_SECTION)
        .expect("an app it takes reads")
        .expect("an app it takes has a kai.app");
    let text = core::str::from_utf8(section).expect("a kai.app it takes is text");
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("kai-app 1"));
    let mut ids = 0;
    for line in lines {
        match line.split_once(' ') {
            Some(("id", id)) => {
                assert_eq!(id, name);
                ids += 1;
            }
            Some(("abi", _)) => {}
            _ => panic!("{name}'s kai.app has a line {line:?}"),
        }
    }
    assert_eq!(ids, 1, "{text:?}");
}
