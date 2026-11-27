//! The release boot (P4 task 19's exit case): `release-image --test-root`'s
//! bytes as the one boot disk, no `kai-data` disk of its own (the image's
//! partition 5 is it) and no `test=` mode, so no "ready for keys" and no
//! isa-debug-exit. `release_boot.rs` boots it; `cargo xtask test` never does.
//!
//! A person's first steps on a fresh install: R installs the release in
//! `kai-a` and `kai-b` and resets (`release_boot.rs` lets the VM restart),
//! then on `kai-a` the desk comes up, Ctrl+Alt+F12
//! gives the console the screen (the keys wait for `slots: up in kai-a`, a
//! line only that boot prints, since R's own supervisor can print
//! `desktop: claimed` just before its reset and keys sent then reach `kai-a`
//! before its display claim, where they are lost with no log line), `models import` asks once, and `y` allows
//! it. The import line, with the model's id, hash and length, is added by
//! `release_boot.rs` from the catalog.

use super::{TestCase, case};

/// What `pkg` would print had the image no packages to install: a release
/// installs its built-in apps (`release_boot.rs` expects them kept).
pub const NO_PACKAGES: &str = "generation 1 is its packages: no packages";

/// The lines no release boot may print: every case's
/// ([`super::FORBID`]), and [`NO_PACKAGES`].
const FORBID: &[&str] = &[
    "pkg: FAIL",
    "cannot ask yet",
    "dropped a result",
    "did not reach the app",
    "supervision: supervisor died",
    super::ROOM_WARNING,
    NO_PACKAGES,
];

/// The line only the `kai-a` boot prints, after its display claim and just
/// before boot-ok: the first keys wait for it, not for `desktop: claimed`.
pub const KAI_A_UP: &str = "slots: up in kai-a";

/// "models import" and Enter.
const MODELS_IMPORT: &[&str] = &[
    "~2000", "m", "o", "d", "e", "l", "s", "spc", "i", "m", "p", "o", "r", "t", "ret",
];

pub const RELEASE_BOOT: TestCase = TestCase {
    keys: &[&["~1000", "ctrl-alt-f12"], MODELS_IMPORT, &["~1000", "y"]],
    keys_on: &[
        KAI_A_UP,
        "desktop: lost the screen to the console",
        "1 Allow",
    ],
    expect: &[
        "slots: in R with no record: installed release ",
        KAI_A_UP,
        "desktop: claimed",
        "Importing qwen3-0.6b (",
    ],
    stops: true,
    server: false,
    forbid: FORBID,
    ..case("release-boot", "")
};
