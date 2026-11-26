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
            "ui-window-show: the console took the screen and gave it back",
            "comphost: notes-window napped, unused",
            "ui-window-show: the first show answered ok shown",
            "comphost: desktop had forgotten it; its windows open anew",
            "comphost: shown again, as it was",
            "ui-window-show: a closed window stayed closed until asked, then came back with its note",
        ],
        forbid: SHOW_FORBID,
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
            "is on the desk as w",
            "desk-reopen: a show after the second open answered ",
            "desk-reopen: Notes closed from its control opened again",
        ],
        forbid: REOPEN_FORBID,
        ..case("desk-reopen", "test=desk-reopen")
    },
    // Kai Desk PR 8: with the Terminal holding the keys on a lease (every
    // boot), a Super tap still reaches desktop and ends the lease; Start
    // opens, "not" selects Notes, the first app whose name starts with it,
    // and Enter opens it with `open notes`, never as a request for the model
    // (`kernel/src/start_tests.rs`). desk-start-esc does it with Ctrl+Esc.
    TestCase {
        memory_mib: Some(1024),
        expect: &[
            "desk-start: the terminal holds the keys",
            "desk-start: a Super tap reached desktop through the lease",
            "desktop: focus Panel",
            "desktop: the Apps sheet lists ",
            "desktop: the Apps sheet sent \"open notes\"",
            "desk-start: \"not\" and Enter opened Notes from Start",
        ],
        forbid: START_FORBID,
        ..case("desk-start", "test=desk-start")
    },
    TestCase {
        memory_mib: Some(1024),
        expect: &[
            "desk-start-esc: the terminal holds the keys",
            "desk-start-esc: Ctrl+Esc reached desktop through the lease",
            "desktop: the Apps sheet sent \"open notes\"",
            "desk-start-esc: \"not\" and Enter opened Notes from Start",
        ],
        forbid: START_FORBID,
        ..case("desk-start-esc", "test=desk-start-esc")
    },
];

/// [`FORBID`], and a request for the model: Start sends an app's line, never
/// the text typed to find it.
const START_FORBID: &[&str] = &[
    "pkg: FAIL",
    "cannot ask yet",
    "dropped a result",
    "did not reach the app",
    "supervision: supervisor died",
    ROOM_WARNING,
    "desktop: the panel queued a request",
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

/// `ui-window-show`'s: [`FORBID`], and comphost opening its windows a second
/// time after the console had the screen (H3-26).
const SHOW_FORBID: &[&str] = &[
    "pkg: FAIL",
    "cannot ask yet",
    "dropped a result",
    "did not reach the app",
    "supervision: supervisor died",
    ROOM_WARNING,
    "reopened its windows after desktop restarted",
];
