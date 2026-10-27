//! U, updates and the trust root: `kaituf`, `bootslot`, `updated` (ADR 0008).
//!
//! The boot lever on the R/A/B slot disk. One case boots
//! ten times on one variable store, each boot a `then` of the one before, and
//! every boot must land where `bootslot` says: a fresh install in R, A's first
//! trial boot and commit, B staged, B failing all three of its tries through R,
//! and A again, saying what happened to B.

use super::{TestCase, case};

/// A slot boot: ends in a reset, which QEMU reports as status 0 under `-no-reboot`.
const fn boot(
    name: &'static str,
    cmdline: &'static str,
    expect: &'static [&'static str],
) -> TestCase {
    TestCase {
        expect,
        server: false,
        resets: true,
        slots: true,
        ..case(name, cmdline)
    }
}

const A_AGAIN: TestCase = boot(
    "slot-ab (10: back on kai-a)",
    "test=slot panic=reset",
    &[
        "slots: booted kai-a (record read)",
        "slots: up in kai-a (committed, 0 tries left)",
        "slots: the last boot of kai-b faulted (kind 1)",
        "slots: next boot kai-a",
    ],
);
const R_GIVES_UP_ON_B: TestCase = TestCase {
    then: Some(&A_AGAIN),
    ..boot(
        "slot-ab (9: R moves to kai-a)",
        "test=slot panic=reset",
        &[
            "slots: booted recovery (record read)",
            "slots: R arms kai-a (committed)",
        ],
    )
};
const B_FAILS_3: TestCase = TestCase {
    then: Some(&R_GIVES_UP_ON_B),
    ..boot(
        "slot-ab (8: kai-b fails a third time)",
        "test=slot slot-do=panic panic=reset",
        &[
            "slots: booted kai-b",
            "slots: fault recorded in kai-b; R boots next",
        ],
    )
};
const R_RETRIES_B_2: TestCase = TestCase {
    then: Some(&B_FAILS_3),
    ..boot(
        "slot-ab (7: R tries kai-b again)",
        "test=slot panic=reset",
        &["slots: R arms kai-b (trial)"],
    )
};
const B_FAILS_2: TestCase = TestCase {
    then: Some(&R_RETRIES_B_2),
    ..boot(
        "slot-ab (6: kai-b fails again)",
        "test=slot slot-do=panic panic=reset",
        &[
            "slots: booted kai-b",
            "slots: fault recorded in kai-b; R boots next",
        ],
    )
};
const R_RETRIES_B: TestCase = TestCase {
    then: Some(&B_FAILS_2),
    ..boot(
        "slot-ab (5: R tries kai-b again)",
        "test=slot panic=reset",
        &[
            "slots: booted recovery (record read)",
            "slots: R arms kai-b (trial)",
        ],
    )
};
const B_FAILS: TestCase = TestCase {
    then: Some(&R_RETRIES_B),
    ..boot(
        "slot-ab (4: kai-b fails before its checks)",
        "test=slot slot-do=panic panic=reset",
        &[
            "slots: booted kai-b (record read)",
            "slots: fault recorded in kai-b; R boots next",
        ],
    )
};
const A_STAGES_B: TestCase = TestCase {
    then: Some(&B_FAILS),
    ..boot(
        "slot-ab (3: kai-a stages kai-b)",
        "test=slot slot-do=stage slot-seq=2 panic=reset",
        &[
            "slots: up in kai-a (committed, 0 tries left)",
            "slots: staged kai-b as release 2; restarting into it",
            "slots: next boot kai-b",
        ],
    )
};
const A_COMMITS: TestCase = TestCase {
    then: Some(&A_STAGES_B),
    ..boot(
        "slot-ab (2: kai-a commits)",
        "test=slot slot-do=commit panic=reset",
        &[
            "slots: booted kai-a (record read)",
            "slots: up in kai-a (trial, 2 tries left)",
            "slots: committed kai-a",
            "slots: next boot kai-a",
        ],
    )
};

/// The first boot of a fresh slot disk: R installs and arms A.
const INSTALL: &[&str] = &[
    "slots: booted recovery (record missing)",
    "slots: in R with no record: installed release 1 in kai-a and kai-b as trials",
    "slots: R arms kai-a (trial)",
    "slots: next boot kai-a",
];

/// `update-stage` boot 3: B, written by `updated`, boots as its trial.
const B_UP: TestCase = boot(
    "update-stage (3: kai-b boots its trial)",
    "test=slot panic=reset",
    &[
        "slots: booted kai-b (record read)",
        "slots: up in kai-b (trial, 2 tries left)",
        "slots: next boot kai-b",
    ],
);
/// `update-stage` boot 2: `updated` in A checks the Update calls, writes B and
/// restarts into it.
const A_UPDATES: TestCase = TestCase {
    then: Some(&B_UP),
    ..boot(
        "update-stage (2: updated stages kai-b)",
        "test=update panic=reset",
        &[
            "slots: up in kai-a (trial, 2 tries left)",
            "updated: slots R ",
            "this boot runs in A",
            "updated: SLOT_WRITE refuses the active slot, and R from a trial slot",
            "updated: BOOTREC_SET refuses a bad record and arming a slot being written",
            "updated: staged B as release 2: ",
            "update: restarting into kai-b",
        ],
    )
};

pub(super) const CASES: &[TestCase] = &[
    // Spikes S08-3 and S11-1: P-256 and Ed25519 verification, a 1,000-target
    // `packages` role and SHA-256 in soft-float ring 3, reported in the log.
    TestCase {
        expect: &["tuf-bench: done"],
        server: false,
        ..case("tuf-bench", "test=tuf-bench")
    },
    TestCase {
        then: Some(&A_COMMITS),
        ..boot("slot-ab", "test=slot panic=reset", INSTALL)
    },
    // The Update kind (ADR 0008 §10): `updated` alone reaches the slots and the
    // boot record, and stages the slot that is not running.
    TestCase {
        then: Some(&A_UPDATES),
        ..boot("update-stage", "test=slot panic=reset", INSTALL)
    },
];
