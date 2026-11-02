//! AP, the apps (P4): one case for each built-in app. The Terminal's and the
//! model manager's are driven by the agent through the mock model, which answers
//! a request by its text; the Monitor's boots the app's plan line on the desktop
//! and watches its window through comphost's and `desktop`'s log lines
//! (`kernel/src/app_window_tests.rs`).

use super::{TestCase, case};
use crate::blklog::Data;
use crate::disk::Fixture;

/// A fresh `kai-data` disk, for the model store.
const DATA: Data = Data {
    nvme: false,
    log: None,
    gpt: true,
};

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
    // P4 task 10: the agent manages the models through the `model_*` rows. The
    // store holds `fixture` as the model in use; the import disk holds
    // `fixture-2`, the image's role's other test target, which the import
    // stores only once its length and SHA-256 are the target's.
    TestCase {
        expect: &[
            "block: kai-import disk holds a GGUF file; a disk with a role is never the model",
            "updated: imported fixture-2 (cc73ee264158), 2105341 bytes",
            "app-models: the agent listed the models, imported fixture-2 checked against the signed role, used it, and undo put fixture back",
        ],
        data: Some(DATA),
        store: Some(Fixture::Gguf),
        import: Some(Fixture::Gguf2),
        ..case("app-models", "test=app-models")
    },
];
