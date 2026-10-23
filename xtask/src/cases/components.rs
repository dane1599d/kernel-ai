//! C, components: live editing, the edit loop and the boot manifest.

use super::{TestCase, case};
use crate::disk::Fixture;
use std::time::Duration;

pub(super) const CASES: &[TestCase] = &[
    case("live", "test=live"),
    TestCase {
        expect: &[
            "edit: the edit service restarted; restored notes e1 with 1 edit",
            "edit: the edit service restarted; restored notes 2 with 1 edit",
            "[edit] built e3 from 2 (commit ",
        ],
        build: true,
        after: Some(crate::buildsvc::verify::after_edit_case),
        min_timeout: Some(Duration::from_secs(300)),
        ..case("edit", "test=edit")
    },
    TestCase {
        build: true,
        prepare: Some(crate::boot_smoke::prepare_manifest),
        after: Some(crate::boot_smoke::after_manifest),
        min_timeout: Some(Duration::from_secs(300)),
        ..case("manifest", "test=manifest")
    },
    TestCase {
        build: true,
        prepare: Some(crate::boot_smoke::prepare_manifest_good),
        after: Some(crate::boot_smoke::after_manifest_good),
        min_timeout: Some(Duration::from_secs(300)),
        ..case("manifest-good", "test=manifest-good")
    },
    // The live scenario with the model on the disk and no server: `:why` is
    // answered by the local model.
    TestCase {
        disk: Some(Fixture::LiveLocal),
        server: false,
        expect: &["live: :why notes was answered by the local model"],
        ..case("live-local", "test=live")
    },
];
