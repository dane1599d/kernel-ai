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
    // Kai Desk PR 8's taskbar by clicks, as `desktop` says it did each one
    // (`kernel/src/taskbar_tests.rs`): the Notes pin sends `open notes` with
    // no model; its item minimizes and restores; the Kai button opens Start;
    // the Kai chip opens the pane, a maximized window stops 12 px left of it
    // (`MAXIMIZED_BESIDE_PANE`), and fits the work area again once it closes.
    TestCase {
        memory_mib: Some(1024),
        expect: &[
            "desk-taskbar: clicked the Notes pin",
            "desktop: the taskbar sent \"open notes\"",
            "desktop: window 2 \"Notes\" opened at ",
            "desktop: window 2 minimized",
            "desktop: window 2 restored",
            "desktop: the Apps sheet lists ",
            "desktop: the Kai pane opened",
            MAXIMIZED_BESIDE_PANE,
            "desktop: the Kai pane closed",
            MAXIMIZED_ALONE,
            "desk-taskbar: the taskbar opened, minimized, restored and maximized beside the pane",
        ],
        forbid: START_FORBID,
        ..case("desk-taskbar", "test=desk-taskbar")
    },
];

/// Notes' maximized frame with the pane open at 1280x800: the work area left
/// of the pane, 12 px short of it (the pane is 360 wide, 12 from the right).
const MAXIMIZED_BESIDE_PANE: &str = "desktop: window 2 moved to 0,0 size 896x752";
/// And with the pane closed again: the whole work area above the taskbar.
const MAXIMIZED_ALONE: &str = "desktop: window 2 moved to 0,0 size 1280x752";

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

#[cfg(test)]
mod tests {
    use super::*;
    use deskgeom::{Layout, UiState};

    /// The case's expected frames are deskgeom's maximized rectangles at
    /// 1280x800, with the pane open and closed.
    #[test]
    fn the_taskbar_cases_frames_are_the_layouts_maximized_rectangles() {
        let frame = |pane_open| {
            let r = Layout::for_screen(1280, 800, UiState { pane_open }).maximized;
            format!(
                "desktop: window 2 moved to {},{} size {}x{}",
                r.x, r.y, r.w, r.h
            )
        };
        assert_eq!(frame(true), MAXIMIZED_BESIDE_PANE);
        assert_eq!(frame(false), MAXIMIZED_ALONE);
        // Left of the pane by the flyout margin.
        let l = Layout::for_screen(1280, 800, UiState { pane_open: true });
        assert_eq!(l.maximized.w, l.pane.x - deskgeom::FLYOUT_MARGIN);
    }
}
