//! Backlog item 2 (the founder's second test, lane C): a closed app comes
//! back when asked, and not before.

use super::{ROOM_WARNING, TestCase, case};

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
    // The founder-walk (#922): Notes opened by "open notes", closed from its
    // control, and "open notes" again. The shell's `kai-show 1` goes through
    // the supervisor and the kernel's APP_CALL, which refused it (Invalid)
    // (`kernel/src/agent_desk_tests.rs`).
    TestCase {
        // `fetch`, in every image since the shell routes to it, left the
        // default 512 MiB too little to open Notes (`desk-agent`).
        memory_mib: Some(1024),
        keys: &[
            &["o", "p", "e", "n", "spc", "n", "o", "t", "e", "s", "ret"],
            &["o", "p", "e", "n", "spc", "n", "o", "t", "e", "s", "ret"],
        ],
        expect: &[
            "desk-reopen: ui_open started Notes and its window opened",
            "comphost: window 1 hidden until it is shown",
            "desk-reopen: Notes' window was closed from its control",
            "comphost: shown again, as it was",
            "desk-reopen: a show after the second open answered ",
            "desk-reopen: Notes closed from its control opened again",
        ],
        forbid: REOPEN_FORBID,
        ..case("desk-reopen", "test=desk-reopen")
    },
];

/// [`FORBID`], and the shell's lines for a show refused or a window that never
/// came.
const REOPEN_FORBID: &[&str] = &[
    "pkg: FAIL",
    "cannot ask yet",
    "dropped a result",
    "did not reach the app",
    "supervision: supervisor died",
    ROOM_WARNING,
    "was asked to show its window",
    "showed no window",
];
