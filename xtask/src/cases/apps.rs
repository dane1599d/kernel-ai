//! AP, the apps (P4): one case for each built-in app. The Terminal's is driven by
//! the agent through the mock model, which answers a request by its text; the
//! Monitor's boots the app's plan line on the desktop and watches its window
//! through comphost's and `desktop`'s log lines (`kernel/src/app_window_tests.rs`).

use super::{TestCase, case};

pub(super) const CASES: &[TestCase] = &[
    // P4 task 4: the AI types into the Terminal app, a person's Enter runs the line
    // (the harness's one key), and the AI reads the rows.
    TestCase {
        keys: &[&["ret"]],
        expect: &[
            "app-terminal: the AI typed a line, a person's Enter ran it, and the AI read its reply",
        ],
        ..case("app-terminal", "test=app-terminal")
    },
    // P4 task 9: the System monitor's window on the desktop, given a sample of
    // the machine by comphost (granted by the plan line's name), then closed.
    TestCase {
        expect: &[
            "comphost: window 1 \"System Monitor\" shown at 640x440",
            "desktop: window 1 \"System Monitor\" opened at 48,52 size 642x474",
            "comphost: the first system sample reached the monitor",
            "comphost: window 1 closed",
            "desktop: window 1 closed",
            "ui-monitor: the Monitor's window was shown, and its close control pressed",
        ],
        ..case("ui-monitor", "test=ui-monitor")
    },
    // P4 task 5: the Files window on the desktop, shown and closed from its
    // control. No shell runs, so what it asks the OS for waits unanswered.
    TestCase {
        expect: &[
            "comphost: window 1 \"Files\" shown at 720x460",
            "desktop: window 1 \"Files\" opened at 48,52 size 722x494",
            "comphost: window 1 closed",
            "desktop: window 1 closed",
            "ui-files: the Files window was shown, and its close control pressed",
        ],
        ..case("ui-files", "test=ui-files")
    },
];
