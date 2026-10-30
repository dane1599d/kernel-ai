//! AP, the P4 apps that are components: each case boots the app's plan line on the
//! desktop and watches it work through comphost's and `desktop`'s log lines.
//! The kernel side is `kernel/src/app_tests.rs`.

use super::{TestCase, case};

pub(super) const CASES: &[TestCase] = &[
    // P4 task 9: the System monitor's window on the desktop, given a sample of
    // the machine by comphost (granted by the plan line's name), then closed.
    TestCase {
        expect: &[
            "comphost: window 1 \"System Monitor\" shown at 640x440",
            "desktop: window 1 \"System Monitor\" opened at 48,52 size 642x474",
            "comphost: the first system sample reached the monitor",
            "comphost: window 1 closed",
            "desktop: window 1 closed",
            "app-monitor: the Monitor's window was shown, and its close control pressed",
        ],
        ..case("app-monitor", "test=app-monitor")
    },
];
