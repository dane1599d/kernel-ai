//! `desk-sends` (ADR 0005 §7, "Amended 2026-11-19"): a send typed in the Kai
//! pane at 1920x1080, where its card and the pane fit side by side
//! (`kernel/src/desk_sends_tests.rs`). The card shows only the call with the
//! text's digest; the text shows in the pane, and the card takes no Allow
//! until all of it has been on the screen; the Allow holds the send 5 s, and
//! `undo` within the hold means it is never sent.

use super::{TestCase, case};

pub(super) const CASES: &[TestCase] = &[TestCase {
    // `display=` needs virtio-gpu; 1920x1080 as `desk-perf` boots it.
    gpu: true,
    memory_mib: Some(2048),
    // The undo must land within the 5 s hold: under TCG it would flake.
    kvm_only: true,
    expect: &[
        "desktop: drew the desk at 1920x1080",
        "is a send's: it waits for its text",
        // The mock model's text: 150 sentences on one line
        // (`xtask/src/mock_llm/sends.rs`).
        "shows its text in the pane (3299 bytes)",
        "desk-sends: the card shows only the call, with the text's digest",
        "desk-sends: y before the text was read through did not allow it",
        "desktop: the send's text scrolled",
        "'s whole text has been on the screen",
        "desk-sends: the card allowed after ",
        "answered allow on desktop's card",
        "a send to hooks.example is held 5000 ms",
        "desk-sends: read through, y allowed the send, held 5 s with undo ",
        "to hooks.example cancelled by undo",
        "desk-sends: undo within the hold cancelled the send, and it was not sent",
    ],
    ..case("desk-sends", "test=desk-sends display=1920x1080")
}];
