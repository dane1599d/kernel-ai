//! The founder's walkthrough (founder test 2, test 6): lines a first-time user
//! typed, report only for now and gated once stable. A file of its own, so the
//! gated arm's `tasks_sha256` (a hash of `tasks.rs`) does not move with it.
//!
//! "what time is it", "what can you do with the cpus" and a long line typed
//! after a clarifying question are meant to be answered by the shell's code
//! before the model (#893, #865, #862); until those land, the model answers
//! them, and they are left out here.

use super::tasks::Cat::Single;
use super::tasks::Expect::{Clarify, Name};
use super::tasks::{Expect, Task};

const fn line(line: &'static str, expect: &'static [Expect]) -> Task {
    Task {
        cat: Single,
        line,
        state: "",
        history: &[],
        expect,
    }
}

pub const FOUNDER: &[Task] = &[
    line("open terminal", &[Name("ui_open")]),
    line("open the file manager", &[Name("ui_open")]),
    line(
        "make a note called ideas and type hello dane into it",
        &[Name("notes_add")],
    ),
    // The founder's line as typed, first letter and all.
    line(
        "Smake a note called ideas and type hello dane into it",
        &[Name("notes_add")],
    ),
    line("add a note saying hello dane", &[Name("notes_add")]),
    line(
        "open ideas.txt in the editor",
        &[Name("ui_open"), Name("editor_open_file")],
    ),
    line("dark mode please", &[Name("appearance_set")]),
    line("remove the file ideas", &[Name("fs_trash")]),
    // A stray line: a question back, or a start or stop of the name the bench
    // world does not have, which the shell's door turns into one.
    line(
        "sssssshd",
        &[
            Clarify,
            Name("service_start"),
            Name("service_stop"),
            Name("process_stop"),
        ],
    ),
];
