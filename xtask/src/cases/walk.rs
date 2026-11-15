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
                "\"open terminal\", opened by code with no model",
                // Where windows open is deskgeom's, which the kernel's clicks
                // follow: the host checks only which window each one is.
                "desktop: window 1 \"Terminal\" opened at ",
                "founder-walk: the Terminal opened from the panel",
                // The line as typed, every key of it, ran in the Terminal.
                "terminal-window ran the line \"show me the cpus please\"",
                "founder-walk: the Terminal ran the typed line and showed the CPUs",
                "answered from the registry",
                "founder-walk: what Kai can do with the cpus came from the rows",
                "founder-walk: the model's clarify offered only what a row backs",
                // The line after the clarify was a request of its own.
                "the question about \"do more with the cpus\" was left: \"save a note for the terminal\" reads as a new request",
                // The file is not in the line, so the call was added: it asks.
                "fs_create is ask (Added)",
                "card: the shell's waiting card goes to desktop",
                " answered yes",
                // What the card came to shows in the Terminal's row, which the
                // kernel reads; the panel's bar is the panel's own lines'.
                "founder-walk: the Terminal's line asked first, Run on the card made its file, and the Terminal showed what it came to",
                "desktop: the panel shows what a card came to",
                // What Run on the panel's card came to: code's done line,
                // without its undo (`shellcore::deskask`). `fs_create` never
                // overwrites, so nothing made the file before the answer.
                "desktop: the bar says \"Done: file walk-panel.txt created (was absent).\"",
                "founder-walk: the panel's line asked first, and Run on the card made its file",
                "founder-walk: the note reached Notes on a fresh boot",
                // Notes is the desk's second window, whose close control the
                // walk clicks at deskgeom's point for it.
                "desktop: window 2 \"Notes\" opened at ",
                "comphost: window 1 hidden until it is shown",
                "comphost: shown again, as it was",
                "founder-walk: Notes' window came back when asked again",
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
