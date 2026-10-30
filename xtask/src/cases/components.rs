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
    // P4 task 11: the editor app. The agent's door opens Notes' source at a line in
    // the editor's window; once the case has clicked into it the host types a
    // comment (`// src/lib.rs:6`) and Ctrl-B, and the edit builds in the background;
    // then it clicks on that link with Ctrl held, and the editor shows line 6.
    TestCase {
        keys: &[
            &[
                "slash",
                "slash",
                "spc",
                "s",
                "r",
                "c",
                "slash",
                "l",
                "i",
                "b",
                "dot",
                "r",
                "s",
                "shift-semicolon",
                "6",
                "ctrl-b",
            ],
            // Ctrl held for the case's click on the link, which it makes inside the hold.
            &["!sendkey ctrl 8000"],
        ],
        expect: &[
            "app-editor: the agent opened Notes' src/lib.rs at line 1 in the editor",
            "[edit] built e",
            "app-editor: an edit typed in the editor was built in the background",
            "app-editor: a Ctrl-click on a source link in the editor opened its line",
        ],
        build: true,
        min_timeout: Some(Duration::from_secs(300)),
        // The page budget is half the free frames: with `desktop`, Notes, the edit
        // service and a second comphost the 512 MiB base leaves the editor out.
        memory_mib: Some(1024),
        ..case("app-editor", "test=app-editor")
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
    // S03-1's deferred figures: the view rebuild and layout pass in hard-float
    // ring 3, reported in the log.
    TestCase {
        expect: &["appbench: done"],
        server: false,
        ..case("appbench", "test=appbench")
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
