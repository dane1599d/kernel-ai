//! Backlog item 2 (the founder's second test, lane C): a closed app comes
//! back when asked, and not before.

use super::{TestCase, case};

pub(super) const CASES: &[TestCase] = &[
    // Notes' window closed from its control: hidden by comphost. It naps, a
    // note added wakes and redraws it, and it stays hidden; `kai-show 1` puts
    // it back as it was (`kernel/src/show_tests.rs`).
    TestCase {
        expect: &[
            "comphost: window 1 hidden until it is shown",
            "ui-window-show: Notes' window was closed",
            "comphost: notes-window napped, unused",
            "ui-window-show: the first show answered ok shown",
            "comphost: shown again, as it was",
            "ui-window-show: a closed window stayed closed until asked, then came back with its note",
        ],
        ..case("ui-window-show", "test=ui-window-show")
    },
];
