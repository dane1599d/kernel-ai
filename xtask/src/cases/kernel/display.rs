//! K's display cases: the screen claimed, and windows opened, moved, restarted and asked over.

use super::{TestCase, case};

pub(in crate::cases) const CASES: &[TestCase] = &[
    // P3 task 2: desktop claims the screen; Ctrl+Alt+F12 switches it.
    TestCase {
        expect: &[
            "desktop: claimed",
            "desktop: key 0x4 1",
            "desktop: lost the screen to the console",
            "desktop: the screen is back",
            "display-claim: a full input queue starts the next read with INPUT_LOST",
            "desktop: the Kai pane came back with a 3-character line",
            "display-claim: desktop claimed the screen, Ctrl+Alt+F12 gave it to the console and back, and only desktop could hold it",
        ],
        ..case("display-claim", "test=display-claim")
    },
    // P3 task 6: windows open, move and close.
    TestCase {
        expect: &[
            "deskdemo: opened Left and Right",
            "desktop: window 1 \"Left\" opened at 48,52 size 302x234",
            "desktop: window 2 \"Right\" opened at 80,84 size 302x234",
            "desktop: window 1 has focus",
            "desktop: window 1 moved to 248,52 size 302x234",
            "deskdemo: closed Left",
            "desktop: window 1 closed",
            "desktop: window 2 has focus",
            "desk-windows: dragged Left 200 px and pressed its close control",
        ],
        ..case("desk-windows", "test=desk-windows")
    },
    // P3 task 6: desktop killed and restarted; its client reconnects.
    TestCase {
        expect: &[
            "deskdemo: opened Left and Right",
            "display: its holder ended; the console has the screen",
            "deskdemo: reopened Left and Right after desktop restarted",
            "desk-restart: desktop was killed and came back, and deskdemo kept running",
        ],
        ..case("desk-restart", "test=desk-restart")
    },
    // P3 task 8: an ask is answered only on the console, whatever the lease.
    TestCase {
        keys: &[
            &["y", "ret"],
            &["n"],
            &["y", "ret"],
            &["n"],
            &["n"],
            &["y", "ret"],
        ],
        expect: &[
            "display: the console took the screen for an answer",
            "desk-ask: an ask after keys leased to the console was answered only on the console",
            "desk-ask: an ask after keys with desktop was answered only on the console",
            "display: the answer came; the screen went back to its holder",
            "desk-ask: with the console showing already, the ask kept the screen until its answer",
            "desk-ask: a screen handed back just before an ask went back after its answer",
            "desk-ask: the restarted desktop got the keys, with no lease left over",
        ],
        ..case("desk-ask", "test=desk-ask")
    },
];
