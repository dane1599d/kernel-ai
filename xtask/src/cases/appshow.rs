//! Backlog item 2 (the founder's second test, lane C): a closed app comes
//! back when asked, and not before.

use super::{TestCase, case};

pub(super) const CASES: &[TestCase] = &[
    // Notes' window closed from its control; a note added redraws it, and it
    // stays closed; `kai-show 1` reloads it and its window comes back
    // (`kernel/src/show_tests.rs`).
    TestCase {
        expect: &[
            "ui-window-show: Notes' window was closed",
            "ui-window-show: the first show answered ok shown notes-window",
            "notes-window: reloaded to show its window",
            "ui-window-show: a closed window stayed closed until asked, then came back with its note",
        ],
        ..case("ui-window-show", "test=ui-window-show")
    },
];
