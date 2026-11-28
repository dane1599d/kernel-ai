//! The key-rotation drill as boot cases (P8 row 3, throwaway test roots only;
//! `image/update_disk.rs`). Every slot trusts test root v1. An update disk
//! signed under v2, with `2.root.json` signed by both roots' keys, moves the
//! device to v2 and installs its release. From then the cached v2 wins over
//! the v1 root each slot carries, and a disk signed under v1 alone is refused.
//! A rotation that only v2's root key signed is refused from the start.

use super::updates::{DATA, INSTALL, boot};
use super::{TestCase, UpdateDisk};

/// Boot 2 of both cases: A, trusting v1, takes the disk signed under v2,
/// moves to root version 2 and writes release 2 into B.
const fn rotates(name: &'static str, then: &'static TestCase) -> TestCase {
    TestCase {
        then: Some(then),
        data: Some(DATA),
        update: Some(UpdateDisk::ROOT2),
        ..boot(
            name,
            "test=update updated=update panic=reset",
            &[
                "updated: the trusted root is the slot's own, version 1",
                "updated: the update disk's repository verifies: slot.img is release 2, ",
                "updated: wrote release 2 into B and checked its sha256",
                "updated: the verified metadata is in /system/trust",
                "update: restarting into kai-b",
            ],
        )
    }
}

/// `update-root-rotate` boot 3: B boots release 2 and starts from root
/// version 2, kept in `/system/trust`, not its own v1. The same disk again is
/// refused only as no newer release.
const ROTATED_B: TestCase = TestCase {
    data: Some(DATA),
    update: Some(UpdateDisk::ROOT2),
    ..boot(
        "update-root-rotate (3: kai-b boots release 2 on root v2)",
        "test=update updated=update panic=reset",
        &[
            "slots: booted kai-b (record read)",
            "updated: the trusted root is /system/trust's, version 2",
            "updated: FAIL refused: release 2 is not newer than 2 (anti-rollback)",
            "updated: restarting into the running slot",
        ],
    )
};
const ROTATES: TestCase = rotates("update-root-rotate (2: kai-a moves to root v2)", &ROTATED_B);

/// `update-root-old` boot 3: after the move, a disk signed under v1 alone is
/// refused: its timestamp is not signed by v2's timestamp key.
const OLD_REFUSED: TestCase = TestCase {
    data: Some(DATA),
    update: Some(UpdateDisk::ROOT1),
    ..boot(
        "update-root-old (3: kai-b refuses a disk signed under v1)",
        "test=update updated=update panic=reset",
        &[
            "slots: booted kai-b (record read)",
            "updated: the trusted root is /system/trust's, version 2",
            "updated: FAIL refused: below the signature threshold: timestamp has 0 valid signatures",
            "updated: restarting into the running slot",
        ],
    )
};
const OLD_ROTATES: TestCase = rotates("update-root-old (2: kai-a moves to root v2)", &OLD_REFUSED);

/// `update-root-one-key` boot 2: A, trusting v1, refuses a disk whose
/// `2.root.json` only v2's root key signed, and stays on v1.
const ONE_KEY_REFUSED: TestCase = TestCase {
    data: Some(DATA),
    update: Some(UpdateDisk::ROOT2_ONE_KEY),
    ..boot(
        "update-root-one-key (2: kai-a refuses a rotation v1 never signed)",
        "test=update updated=update panic=reset",
        &[
            "updated: the trusted root is the slot's own, version 1",
            "updated: FAIL refused: below the signature threshold: root has 0 valid signatures",
            "updated: restarting into the running slot",
        ],
    )
};

pub const CASES: &[TestCase] = &[
    TestCase {
        then: Some(&ROTATES),
        data: Some(DATA),
        ..boot("update-root-rotate", "test=slot panic=reset", INSTALL)
    },
    TestCase {
        then: Some(&OLD_ROTATES),
        data: Some(DATA),
        ..boot("update-root-old", "test=slot panic=reset", INSTALL)
    },
    TestCase {
        then: Some(&ONE_KEY_REFUSED),
        data: Some(DATA),
        ..boot("update-root-one-key", "test=slot panic=reset", INSTALL)
    },
];
