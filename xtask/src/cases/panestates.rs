//! `pane-states`: the Kai pane through each of its states on the default
//! system with the mock model (`kernel/src/pane_states_tests.rs`), read from
//! `desktop`'s lines (`log_bar=1`).

use super::{TestCase, case};
use crate::blklog::Data;
use std::time::Duration;

/// `kai-data` on a virtio disk, as `desk-default` boots it: the asking line's
/// file is saved there.
const VIRTIO: Data = Data {
    nvme: false,
    log: None,
    gpt: true,
};

pub(super) const CASES: &[TestCase] = &[TestCase {
    gpu: true,
    data: Some(VIRTIO),
    memory_mib: Some(2048),
    min_timeout: Some(Duration::from_secs(600)),
    expect: &[
        // Idle, as the pane starts.
        "desktop: the panel starts idle",
        // Thinking, from each line's Enter.
        "desktop: the panel shows thinking from the Enter",
        // Done, for a run-tier line, with no card (the kernel side watches
        // the card while the policy moves).
        "desktop: the bar says \"Done: policy round-robin (was interactive).",
        "pane-states: the run-tier line changed the policy and showed no card",
        // Refused, by code before any model: `c` is a script naming no
        // variable.
        "desktop: the panel shows refused",
        "desktop: the bar says \"Variable not found: c (line 1, position 1)\"",
        "pane-states: the panel was given a line it refuses",
        // Asking: the line's question is on the card, and the bar says so.
        "desktop: the bar says \"it asks first: answer Run or Don't on the card\"",
        "card: the shell's waiting card goes to desktop",
        "desktop: the panel shows what a card came to",
        "desktop: the bar says \"Done: file walk-panel.txt created (was absent).\"",
        "pane-states: the line asked on the card, and Run there made its file",
        "pane-states: every state was shown",
    ],
    ..case("pane-states", "test=pane-states display=1024x768")
}];
