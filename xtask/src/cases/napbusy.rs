//! Backlog item 1 (the founder's second test, lane C): a window in use never
//! naps; one that lost the focus does.

use super::{TestCase, case};

pub(super) const CASES: &[TestCase] = &[
    // `notes-window` with a 3-second nap: ten seconds of keys in its focused
    // window keep it awake; a click on the panel takes the focus away, and
    // it naps (`kernel/src/nap_busy_tests.rs`).
    TestCase {
        expect: &[
            "desk-nap-busy: ten seconds of keys in its window kept the app awake",
            "comphost: notes-window napped, unused",
            "desk-nap-busy: a window in use never napped, and napped once the focus left it",
            "desk-nap-busy: closing the focused window let the app nap",
        ],
        ..case("desk-nap-busy", "test=desk-nap-busy")
    },
];
