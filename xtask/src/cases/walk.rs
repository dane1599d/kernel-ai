//! Founder test 2, backlog test 2 (`founder-walk`): the founder's VirtualBox
//! walk of 2026-11-14, end to end, on the default system with the mock model
//! (`kernel/src/founder_walk_tests.rs`), at 1024x768 and at 1920x1080.

use super::{TestCase, case};
use crate::blklog::Data;
use std::time::Duration;

/// `kai-data` on a virtio disk, as `desk-default` boots it: the walk's files
/// are saved there.
const VIRTIO: Data = Data {
    nvme: false,
    log: None,
    gpt: true,
};

/// The walk at `size`: virtio-gpu, which takes `display=`, and room for the
/// Terminal's and Notes' windows beside the system.
macro_rules! walk {
    ($name:literal, $size:literal) => {
        TestCase {
            gpu: true,
            data: Some(VIRTIO),
            memory_mib: Some(2048),
            min_timeout: Some(Duration::from_secs(900)),
            expect: &[
                concat!("desktop: drew the desk at ", $size),
                "founder-walk: the Terminal opened from the panel",
                // The line as typed, every key of it, ran in the Terminal.
                "terminal-window ran the line \"show me the cpus please\"",
                "founder-walk: the Terminal ran the typed line and showed the CPUs",
                "founder-walk: what Kai can do with the cpus came from the rows",
                "founder-walk: the model's clarify offered only what a row backs",
                // The line after the clarify was a request of its own.
                "the question about \"do more with the cpus\" was left: \"save a note for the terminal\" reads as a new request",
                " answered yes",
                "founder-walk: the Terminal's line asked first, and Run on the card made its file",
                "desktop: the panel shows what a card came to",
                // What Run on the panel's card came to: code's done line,
                // without its undo (`shellcore::deskask`). `fs_create` never
                // overwrites, so nothing made the file before the answer.
                "desktop: the bar says \"Done: file walk-panel.txt created (was absent).\"",
                "founder-walk: the panel's line asked first, and Run on the card made its file",
                "founder-walk: the note reached Notes on a fresh boot",
                // Notes is the desk's second window, its close control where
                // the walk clicks.
                "desktop: window 2 \"Notes\" opened at 80,84 size 642x454",
                "comphost: window 1 hidden until it is shown",
                "comphost: shown again, as it was",
                "founder-walk: the walk is done",
            ],
            ..case($name, concat!("test=founder-walk display=", $size))
        }
    };
}

pub(super) const CASES: &[TestCase] = &[
    walk!("founder-walk", "1024x768"),
    walk!("founder-walk-1080", "1920x1080"),
];
