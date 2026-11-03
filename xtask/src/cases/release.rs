//! The release boot (P4 task 19's exit case): `release-image --test-root`'s
//! bytes as the one boot disk, no `kai-data` disk of its own (the image's
//! partition 5 is it) and no `test=` mode, so no "ready for keys" and no
//! isa-debug-exit. `release_boot.rs` boots it; `cargo xtask test` never does.
//!
//! A person's first steps on a fresh install: the desk comes up, Ctrl+Alt+F12
//! gives the console the screen, `models import` asks once, and `y` allows
//! it. The import line, with the model's id, hash and length, is added by
//! `release_boot.rs` from the catalog.

use super::{TestCase, case};

/// "models import" and Enter.
const MODELS_IMPORT: &[&str] = &[
    "~2000", "m", "o", "d", "e", "l", "s", "spc", "i", "m", "p", "o", "r", "t", "ret",
];

pub const RELEASE_BOOT: TestCase = TestCase {
    keys: &[&["~3000", "ctrl-alt-f12"], MODELS_IMPORT, &["~1000", "y"]],
    keys_on: &[
        "desktop: claimed",
        "desktop: lost the screen to the console",
        "1 Allow",
    ],
    expect: &["desktop: claimed", "Importing qwen3-0.6b ("],
    stops: true,
    server: false,
    ..case("release-boot", "")
};
