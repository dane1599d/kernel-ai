//! #556, the Models window's switch (lane AP-2, after v0.1): a person's keys in
//! the window switch the model through the shell's `model_use` row
//! (`kernel/src/models_window_tests.rs`).

use super::apps::{DATA, WINDOW_AND_FS_MIB};
use super::{TestCase, case};
use crate::disk::Fixture;

pub(super) const CASES: &[TestCase] = &[
    // A person's keys in the Models window switch the model through the
    // shell's `model_use` row, logged as the click, and `undo` switches it
    // back. The store holds `fixture`; `fixture-2` is imported from the import
    // disk at the shell first. A click on the title bar gives the window the
    // keys: Refresh, then fixture-2 and Use (`apps/models-ui/tests/keys.rs`).
    TestCase {
        keys: &[&["tab", "ret"], &["tab", "down", "down", "tab", "ret"]],
        expect: &[
            "updated: imported fixture-2 (cc73ee264158), 2105341 bytes",
            "ui-models-use: fixture-2 was imported at the shell, as a person types it",
            "comphost: the first model switch answer reached the app",
            "ui-models-use: a key in the window switched the model, logged as the person's click",
            "ui-models-use: a person's keys in the Models window switched the model through the shell, and undo switched it back",
            // Off a slot image there is no lever, so updated serves at once.
            "updated: the boot record answered at once",
        ],
        data: Some(DATA),
        store: Some(Fixture::Gguf),
        import: Some(Fixture::Gguf2),
        memory_mib: Some(WINDOW_AND_FS_MIB),
        ..case("ui-models-use", "test=ui-models-use")
    },
];
