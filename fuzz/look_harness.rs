//! The P3 fuzz target's body for `look`'s `kai-theme 1` parser (plan
//! p3-desktop sections 1 and 6: a fuzz target for every new parser; ADR 0005
//! section 8). `fuzz_targets/kai_theme.rs` calls it with libFuzzer's input, and
//! `look/tests/hostile.rs` includes this file and runs it over hostile
//! mutations of the shipped themes, which also seed the target's corpus. It
//! must return for any input; only a bug in the code under it may panic.

#![allow(dead_code)]

use look::theme::{self, Theme};

/// The `kai_theme` target: text read as a theme; one that parses passes its
/// checks and reads back the same.
pub fn kai_theme(bytes: &[u8]) {
    let Ok(text) = core::str::from_utf8(bytes) else {
        return;
    };
    if let Ok(theme) = Theme::parse(text) {
        theme.check().unwrap();
        assert_eq!(Theme::parse(&theme.encode()), Ok(theme));
    }
}

/// The seeds of the target's corpus: every shipped theme.
pub fn seeds() -> Vec<(&'static str, Vec<u8>)> {
    theme::shipped()
        .iter()
        .map(|theme| ("kai_theme", theme.encode().into_bytes()))
        .collect()
}
