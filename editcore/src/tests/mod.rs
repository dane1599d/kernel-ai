//! Host tests, on bundles shaped like the real notes source.

extern crate std;

mod building;
mod command;
mod fake;
mod journal;
mod restart;
mod rules;
mod scenario;
mod screen;
mod screenparts;
mod screenverbs;
mod service;
mod session;
mod wire;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use kabi::live::{MANIFEST_SECTION, SOURCE_SECTION};
use livecore::artifact::{Manifest, append_custom_section, bundle};

pub const LIB: &str = "components/notes/src/lib.rs";
pub const TOML: &str = "components/notes/Cargo.toml";

/// The real notes source, as xtask bundles it: Cargo.toml and every file under
/// src, sorted by path.
pub fn real_notes() -> Vec<u8> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let dir = root.join("components/notes");
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    files.push((
        String::from(TOML),
        std::fs::read(dir.join("Cargo.toml")).unwrap(),
    ));
    for entry in std::fs::read_dir(dir.join("src")).unwrap() {
        let path = entry.unwrap().path();
        let name = format!(
            "components/notes/src/{}",
            path.file_name().unwrap().to_string_lossy()
        );
        files.push((name, std::fs::read(&path).unwrap()));
    }
    files.sort();
    let parts: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    bundle(&parts)
}

/// A small notes-shaped bundle: a manifest and a lib.rs of numbered lines.
pub fn small(lib: &str) -> Vec<u8> {
    bundle(&[
        (TOML, b"[package]\nname = \"comp-notes\"\n"),
        (LIB, lib.as_bytes()),
    ])
}

pub fn numbered(count: usize, ending: &str) -> String {
    (1..=count).map(|n| format!("line {n}{ending}")).collect()
}

/// An artifact of `component` version `version` carrying `source`.
pub fn artifact(component: &str, version: &str, source: &[u8]) -> Vec<u8> {
    let mut module = Vec::from(&b"\0asm\x01\0\0\0"[..]);
    let manifest = Manifest {
        lifecycle: 1,
        component: component.into(),
        version: version.into(),
        world: format!("app:{component}/{component}@1.0.0"),
        engine: "wasmi".into(),
        features: "-".into(),
        licence: "MPL-2.0".into(),
        build: "ab".into(),
        source: "cd".into(),
    };
    append_custom_section(&mut module, MANIFEST_SECTION, manifest.format().as_bytes());
    append_custom_section(&mut module, SOURCE_SECTION, source);
    module
}
