//! K's app lifecycle cases (routes, wakes, suspend, start on demand), and anonymous memory.

use super::{TestCase, case};

pub(in crate::cases) const CASES: &[TestCase] = &[
    // P4: the agent calls an installed app's row through the supervisor.
    TestCase {
        expect: &[
            "app-route: the agent's notes_add reached notes through the supervisor, and undo put it back",
        ],
        ..case("app-route", "test=app-route")
    },
    // P4: a post to the event box wakes the shell, never in place of a line.
    TestCase {
        expect: &[
            "event-wake: 10 terminal lines, each after a post, all came back as lines",
            "event-wake: the ended shell's wake went with it; the new shell reads the box",
        ],
        ..case("event-wake", "test=event-wake")
    },
    // P3 task 11: an app suspended and resumed through its host.
    TestCase {
        expect: &[
            "notes: suspended version 2",
            "notes: resumed version 2",
            "app-suspend: notes gave ",
        ],
        ..case("app-suspend", "test=app-suspend")
    },
    // P3 task 12b: an app starts on its first request, not at boot.
    TestCase {
        expect: &[
            "app-on-demand: notes is not running after boot",
            "notes starts on demand",
            "ipc: notes asked for on demand",
            "app-on-demand: the first request started notes, and it answered",
        ],
        ..case("app-on-demand", "test=app-on-demand")
    },
    // Lane M: anonymous memory comes back on release and on exit.
    TestCase {
        expect: &["mem: 1024 committed pages came back on release and 512 on exit; refusals held"],
        ..case("mem", "test=mem")
    },
];
