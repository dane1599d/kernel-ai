//! U, updates and the trust root: `kaituf`, `bootslot`, `updated` (ADR 0008).
//!
//! The boot lever on the R/A/B slot disk. One case boots
//! ten times on one variable store, each boot a `then` of the one before, and
//! every boot must land where `bootslot` says: a fresh install in R, A's first
//! trial boot and commit, B staged, B failing all three of its tries through R,
//! and A again, saying what happened to B.

use super::{TestCase, UpdateDisk, case};
use crate::blklog::Data;
use crate::disk::Fixture;

/// A `kai-data` disk for `fs`, so `updated` has `/system/trust` and
/// `/system/updates`; one per chained case, kept across its boots.
const DATA: Data = Data {
    nvme: false,
    log: None,
    gpt: true,
};

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
/// `update-stage` boot 2: `updated` in A checks the Update calls, the import
/// disk's among them, writes B and restarts into it. The import disk holds a
/// GGUF file and no disk is `kai-model`, so the kernel must not take it for
/// the model (P4 task 10).
const A_UPDATES: TestCase = TestCase {
    then: Some(&B_UP),
    import: Some(Fixture::Gguf),
    ..boot(
        "update-stage (2: updated stages kai-b)",
        "test=update panic=reset",
        &[
            "slots: up in kai-a (trial, 2 tries left)",
            "updated: slots R ",
            "this boot runs in A",
            "block: kai-import disk holds a GGUF file; a disk with a role is never the model",
            "updated: SLOT_WRITE refuses the active slot, and R from a trial slot",
            "updated: the import disk reads, 4104 sectors of a GGUF file, and SLOT_WRITE refuses it",
            "updated: BOOTREC_SET refuses a bad record and arming a slot being written",
            "updated: staged B as release 2: ",
            "update: restarting into kai-b",
        ],
    )
};

/// `update-disk` boot 5: committed B finds `pending` and clears it, with no
/// restore.
const B_HELD: TestCase = TestCase {
    data: Some(DATA),
    ..boot(
        "update-disk (5: the update held)",
        "test=update updated=reconcile panic=reset",
        &[
            "slots: booted kai-b (record read)",
            "updated: release 2 in B committed; the update held",
            "updated: update snapshot ",
            " is dropped",
            "updated: reconciled: Held",
        ],
    )
};
/// `update-disk` boot 4: B again, its try spent on boot 3, and it commits.
const B_AGAIN: TestCase = TestCase {
    then: Some(&B_HELD),
    data: Some(DATA),
    ..boot(
        "update-disk (4: kai-b commits)",
        "test=slot slot-do=commit panic=reset",
        &[
            "slots: up in kai-b (trial, 1 tries left)",
            "slots: committed kai-b",
        ],
    )
};
/// `update-disk` boot 3: B boots release 2, and the same disk offered again is
/// refused, now from the root `/system/trust` kept.
const B_FROM_DISK: TestCase = TestCase {
    then: Some(&B_AGAIN),
    data: Some(DATA),
    update: Some(UpdateDisk { tampered: false }),
    ..boot(
        "update-disk (3: kai-b boots release 2)",
        "test=update updated=update panic=reset",
        &[
            "slots: booted kai-b (record read)",
            "slots: up in kai-b (trial, 2 tries left)",
            "updated: reconciled: OnTrial",
            "updated: the trusted root is /system/trust's",
            "updated: FAIL refused: release 2 is not newer than 2 (anti-rollback)",
            "updated: restarting into the running slot",
        ],
    )
};
/// `update-disk` boot 2: `updated` in A verifies the update disk and writes B.
const A_FROM_DISK: TestCase = TestCase {
    then: Some(&B_FROM_DISK),
    data: Some(DATA),
    update: Some(UpdateDisk { tampered: false }),
    ..boot(
        "update-disk (2: updated writes kai-b from the disk)",
        "test=update updated=update panic=reset",
        &[
            "updated: the trusted root is the slot's own",
            "updated: the update disk's repository verifies: slot.img is release 2, ",
            "updated: wrote release 2 into B and checked its sha256",
            "updated: the verified metadata is in /system/trust",
            "updated: took update snapshot ",
            "update: restarting into kai-b",
        ],
    )
};
/// `update-tampered` boot 3: A again, and B, refused, cannot boot.
const A_AFTER_REFUSAL: TestCase = boot(
    "update-tampered (3: still kai-a)",
    "test=slot panic=reset",
    &[
        "slots: booted kai-a (record read)",
        "slots: up in kai-a (trial, 1 tries left)",
        "slots: kai-b cannot boot (Updating)",
    ],
);
/// `update-tampered` boot 2: the image on the disk is not the one signed.
const A_REFUSES: TestCase = TestCase {
    then: Some(&A_AFTER_REFUSAL),
    update: Some(UpdateDisk { tampered: true }),
    ..boot(
        "update-tampered (2: updated refuses the image)",
        "test=update updated=update panic=reset",
        &[
            "updated: the update disk's repository verifies: slot.img is release 2, ",
            "updated: FAIL refused: slot.img does not match its signed sha256; B stays unbootable",
            "updated: restarting into the running slot",
        ],
    )
};

/// After probation: the next boot of A finds it committed.
const A_COMMITTED: TestCase = boot(
    "slot-probation (3: kai-a committed)",
    "test=slot panic=reset",
    &["slots: up in kai-a (committed, 0 tries left)"],
);
/// Probation's busy rule: 6 shell requests and 3 s, then a last check.
const A_PROBATION: TestCase = TestCase {
    then: Some(&A_COMMITTED),
    resets: false,
    ..boot(
        "slot-probation (2: kai-a on probation)",
        "test=probation probation_calls=5 probation_ms=3000 probation-lines=6 panic=reset",
        &[
            "slots: up in kai-a (trial, 2 tries left)",
            "probation: sent the shell 6 requests",
            "slots: kai-a passed probation (",
            "probation: kai-a is committed",
        ],
    )
};
/// Probation's idle rule: no requests, so 4 times `probation_ms` and 2 checks.
const A_IDLE: TestCase = TestCase {
    resets: false,
    ..boot(
        "slot-idle (2: an idle kai-a commits)",
        "test=probation probation_calls=1000 probation_ms=1000 panic=reset",
        &[
            "probation: sent the shell 0 requests",
            "slots: kai-a passed probation (0 requests, ",
            "probation: kai-a is committed",
        ],
    )
};

/// A rollback boot of `update-rollback`: slot-disk, on the case's data disk.
const fn rb(
    name: &'static str,
    cmdline: &'static str,
    expect: &'static [&'static str],
    then: Option<&'static TestCase>,
) -> TestCase {
    TestCase {
        then,
        data: Some(DATA),
        ..boot(name, cmdline, expect)
    }
}

/// `update-rollback` boot 10: A, back after B failed all its tries, restores
/// `system` from the snapshot taken before B's first boot.
const RB_RESTORE: TestCase = rb(
    "update-rollback (10: kai-a restores system)",
    "test=update updated=reconcile panic=reset",
    &[
        "slots: booted kai-a (record read)",
        "slots: kai-b cannot boot (Unbootable)",
        "updated: release 2 in B failed before it committed; restored the system volume from update snapshot ",
        "updated: pending is gone after the restore",
        "updated: update snapshot ",
        " is dropped",
        "updated: reconciled: Restored",
    ],
    None,
);
const RB_R3: TestCase = rb(
    "update-rollback (9: R moves to kai-a)",
    "test=slot panic=reset",
    &["slots: R arms kai-a (committed)"],
    Some(&RB_RESTORE),
);
const RB_B3: TestCase = rb(
    "update-rollback (8: kai-b fails a third time)",
    "test=slot slot-do=panic panic=reset",
    &["slots: fault recorded in kai-b; R boots next"],
    Some(&RB_R3),
);
const RB_R2: TestCase = rb(
    "update-rollback (7: R tries kai-b again)",
    "test=slot panic=reset",
    &["slots: R arms kai-b (trial)"],
    Some(&RB_B3),
);
const RB_B2: TestCase = rb(
    "update-rollback (6: kai-b fails again)",
    "test=slot slot-do=panic panic=reset",
    &["slots: fault recorded in kai-b; R boots next"],
    Some(&RB_R2),
);
const RB_R1: TestCase = rb(
    "update-rollback (5: R tries kai-b again)",
    "test=slot panic=reset",
    &["slots: R arms kai-b (trial)"],
    Some(&RB_B2),
);
const RB_B1: TestCase = rb(
    "update-rollback (4: kai-b, release 2, fails before its checks)",
    "test=slot slot-do=panic panic=reset",
    &[
        "slots: booted kai-b (record read)",
        "slots: fault recorded in kai-b; R boots next",
    ],
    Some(&RB_R1),
);
/// `update-rollback` boot 3: committed A takes release 2 from the update disk
/// into B, with the snapshot and `pending` recorded first.
const RB_UPDATE: TestCase = TestCase {
    update: Some(UpdateDisk { tampered: false }),
    ..rb(
        "update-rollback (3: kai-a writes kai-b from the disk)",
        "test=update updated=update panic=reset",
        &[
            "updated: wrote release 2 into B and checked its sha256",
            "updated: took update snapshot ",
            "update: restarting into kai-b",
        ],
        Some(&RB_B1),
    )
};
const RB_COMMIT: TestCase = rb(
    "update-rollback (2: kai-a commits)",
    "test=slot slot-do=commit panic=reset",
    &["slots: committed kai-a"],
    Some(&RB_UPDATE),
);

/// `update-window` boot 3: the restart in the window boots B.
const W_B: TestCase = TestCase {
    data: Some(DATA),
    ..boot(
        "update-window (3: kai-b boots)",
        "test=slot panic=reset",
        &["slots: booted kai-b (record read)"],
    )
};
/// `update-window` boot 2: `updated` stages B without restarting, reads
/// `/system/settings` through its read-only `dir=settings`, then restarts into
/// B inside the restart window (`window=now`, from the current minute) after
/// a heads-up.
const W_STAGE: TestCase = TestCase {
    then: Some(&W_B),
    data: Some(DATA),
    update: Some(UpdateDisk { tampered: false }),
    ..boot(
        "update-window (2: updated waits for the restart window)",
        "test=update updated=update updated.restart=window updated.window=now updated.heads_up_s=2 panic=reset",
        &[
            "updated: wrote release 2 into B and checked its sha256",
            "updated: took update snapshot ",
            "updated: /system/settings has no updates.cbor",
            "updated: release 2 is staged in B; the restart window is ",
            "updated: restarting into B in 2 s, inside the restart window",
            "updated: restarting into B in the restart window",
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
    // Probation (ADR 0008 §5): a trial slot commits after its requests and
    // time with no fault, or, idle, after 4 times the time and 2 checks.
    TestCase {
        then: Some(&A_PROBATION),
        ..boot("slot-probation", "test=slot panic=reset", INSTALL)
    },
    TestCase {
        then: Some(&A_IDLE),
        ..boot("slot-idle", "test=slot panic=reset", INSTALL)
    },
    // An update from the update disk (ADR 0008 §12), checked by kaituf against
    // the slot's root, and the same disk with the image tampered after signing.
    TestCase {
        then: Some(&A_FROM_DISK),
        data: Some(DATA),
        ..boot("update-disk", "test=slot panic=reset", INSTALL)
    },
    // The restart window (P4 question 10): staged, then a restart only inside it.
    TestCase {
        then: Some(&W_STAGE),
        data: Some(DATA),
        ..boot("update-window", "test=slot panic=reset", INSTALL)
    },
    TestCase {
        then: Some(&A_REFUSES),
        ..boot("update-tampered", "test=slot panic=reset", INSTALL)
    },
    // The way back (ADR 0008 §10): a new slot that fails before it commits
    // leaves the old slot to restore `system` from the update snapshot.
    rb(
        "update-rollback",
        "test=slot panic=reset",
        INSTALL,
        Some(&RB_COMMIT),
    ),
];
