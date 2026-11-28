//! The P8 fuzz target's body for livecore's section readers (ADR 0003's plan,
//! P8: fuzzing of the section readers): [`live_sections`] reads the input as
//! an artifact, a module whose custom sections hold the manifest, the source
//! bundle and the actions. `livecore/tests/hostile.rs` includes this file and
//! runs it over hostile mutations of [`artifact`], which also seed the
//! target's corpus; `appabi`'s targets have theirs in `appabi_harness.rs`. It
//! must return for any input; only a bug in the code under it may panic.

#![allow(dead_code)]

use kabi::live::{ACTIONS_SECTION, MANIFEST_SECTION, MAX_ARTIFACT, SOURCE_SECTION};
use livecore::artifact::{
    self, MAX_SOURCE, MAX_SOURCE_FILES, append_custom_section, bundle, custom_section,
    source_files, valid_path,
};
use livecore::manifest::Manifest;

const MAGIC: &[u8] = b"\0asm\x01\0\0\0";

/// Whether `part` lies inside `whole`.
fn within(part: &[u8], whole: &[u8]) -> bool {
    let (start, end) = (
        whole.as_ptr() as usize,
        whole.as_ptr() as usize + whole.len(),
    );
    let at = part.as_ptr() as usize;
    start <= at && at + part.len() <= end
}

/// The files of a bundle that reads: plain paths, each once, no more than
/// the most, and they bundle again to bytes that read as the same files.
fn check_files(files: &[(&str, &[u8])]) {
    assert!(files.len() <= MAX_SOURCE_FILES);
    for (n, (path, _)) in files.iter().enumerate() {
        assert_eq!(valid_path(path), Ok(()), "{path:?}");
        assert!(files[..n].iter().all(|(p, _)| p != path), "{path:?} twice");
    }
    let again = bundle(files);
    assert_eq!(source_files(&again).as_deref(), Ok(files));
}

/// The `live_sections` target: the input is read as a module's custom
/// sections, as a source bundle, and as a whole artifact. A section that is
/// found lies inside the module; a bundle that reads keeps
/// [`check_files`]'s promises; an artifact that reads is within the limits,
/// and its parts, written into a new module, read back the same.
pub fn live_sections(data: &[u8]) {
    for name in [MANIFEST_SECTION, SOURCE_SECTION, ACTIONS_SECTION, ""] {
        if let Ok(Some(payload)) = custom_section(data, name) {
            assert!(within(payload, data), "{name}");
        }
    }
    if let Ok(files) = source_files(data) {
        check_files(&files);
    }
    let Ok(parsed) = artifact::parse(data) else {
        return;
    };
    assert!(data.len() <= MAX_ARTIFACT && parsed.source.len() <= MAX_SOURCE);
    assert!(parsed.actions.is_some() || parsed.manifest.lifecycle < 2);
    assert!(within(parsed.source, data));
    let files = source_files(parsed.source).expect("an artifact's source reads");
    check_files(&files);
    let text = parsed.manifest.format();
    assert_eq!(Manifest::parse(&text).as_ref(), Ok(&parsed.manifest));
    let mut module = MAGIC.to_vec();
    append_custom_section(&mut module, MANIFEST_SECTION, text.as_bytes());
    append_custom_section(&mut module, SOURCE_SECTION, parsed.source);
    if let Some(actions) = parsed.actions {
        append_custom_section(&mut module, ACTIONS_SECTION, actions.as_bytes());
    }
    // The new module can be longer: a manifest is written as version 2.
    if module.len() <= MAX_ARTIFACT {
        let again = artifact::parse(&module).expect("rewritten parts read");
        assert_eq!(again.manifest, parsed.manifest);
        assert_eq!(again.source, parsed.source);
        assert_eq!(again.actions, parsed.actions);
    }
}

/// A `kai-manifest 2` for `lifecycle`.
pub fn manifest(lifecycle: u32) -> String {
    format!(
        "kai-manifest 2\nlifecycle {lifecycle}\ncomponent notes\nversion 3\nworld app:notes/notes@1.0.0\nengine wasmi\nfeatures -\nlicence MIT OR Apache-2.0\nbuild 0123456789abcdef\nsource fedcba9876543210\n"
    )
}

/// An artifact that reads: a type section, then the manifest, two source
/// files and, for `lifecycle 2`, the actions.
pub fn artifact(lifecycle: u32) -> Vec<u8> {
    let mut module = MAGIC.to_vec();
    module.extend_from_slice(&[1, 4, 1, 0x60, 0, 0]);
    append_custom_section(
        &mut module,
        MANIFEST_SECTION,
        manifest(lifecycle).as_bytes(),
    );
    let source = bundle(&[
        ("src/lib.rs", b"fn main() {}"),
        ("Cargo.toml", b"[package]\n"),
    ]);
    append_custom_section(&mut module, SOURCE_SECTION, &source);
    if lifecycle >= 2 {
        append_custom_section(&mut module, ACTIONS_SECTION, b"kai-actions 1\n");
    }
    module
}

/// The corpus seeds: `(target, input)`, the two artifacts and a bare bundle.
pub fn seeds() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("live_sections", artifact(1)),
        ("live_sections", artifact(2)),
        ("live_sections", bundle(&[("a.rs", b"x"), ("b/c.rs", b"")])),
    ]
}
