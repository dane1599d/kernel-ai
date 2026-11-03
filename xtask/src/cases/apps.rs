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

/// `fs` on the data disk, `desktop` and a window together pass the page budget
/// (half the free memory) of the default 512 MiB guest (`keep-running`'s).
const WINDOW_AND_FS_MIB: u64 = 1024;

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
    // P4 task 10: the model manager's window on the desktop, given updated's
    // list by comphost (granted by the plan line's name, and answered by
    // updated for that name alone), then closed. kai-data gives updated its
    // store, which holds no model here, so the list is the role's.
    TestCase {
        expect: &[
            "comphost: window 1 \"Models\" shown at 640x460",
            "desktop: window 1 \"Models\" opened at 48,52 size 642x494",
            "comphost: the first model list reached the window (",
            "comphost: window 1 closed",
            "desktop: window 1 closed",
            "ui-models: the model manager's window was shown, and its close control pressed",
            // Off a slot image there is no lever, so updated serves at once.
            "updated: the boot record answered at once",
        ],
        data: Some(DATA),
        memory_mib: Some(WINDOW_AND_FS_MIB),
        ..case("ui-models", "test=ui-models")
    },
    // P4 task 5: the agent makes, moves and trashes a file, the Files window shows
    // each state, and `undo` puts both back. The OS's rows run at once (a move and
    // a trash have an undo, so they do not ask); the log shows each change.
    TestCase {
        expect: &[
            "app-files: the Files window lists the file the agent made",
            "app-files: after the move the window lists plan.txt and no longer notes.txt",
            "app-files: after the trash the window lists neither file",
            "app-files: undo put the move back, and the window lists notes.txt and not plan.txt",
            "app-files: the agent made, moved and trashed a file at once, the Files window showed each state, and undo put each back; the log showed each change and each undo",
        ],
        data: Some(DATA),
        memory_mib: Some(WINDOW_AND_FS_MIB),
        ..case("app-files", "test=app-files")
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
            // Off a slot image there is no lever, so updated serves at once.
            "updated: the boot record answered at once",
        ],
        data: Some(DATA),
        store: Some(Fixture::Gguf),
        import: Some(Fixture::Gguf2),
        ..case("app-models", "test=app-models")
    },
];
