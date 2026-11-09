//! The OS's own TLS clients (P5 tasks 8 and 10): `pkg` and `updated` reaching
//! their repositories over the network, on a `NetDest` of their own, against
//! xtask's HTTPS mock (`mock_https`) as GitHub serves them, through the harness
//! of P5 task 12 (`cases::https`): the mock's hosts resolve through the mock DNS
//! zone, QEMU forwards port 443 there, and its test CA is the bundle's
//! `netroots/1.der`. No case reaches the internet.

use super::{Https, TestCase, case};
use crate::blklog::Data;
use crate::image::update_online::Fault;
use crate::mock_https::Content;
use crate::pkgrepo::mirrors::MirrorKind;

const VIRTIO: Data = Data {
    nvme: false,
    log: None,
    gpt: true,
};

/// What the harness serves beside the repository the case asks for: nothing.
fn nothing() -> Content {
    Content::new()
}

/// A `pkg-online` boot against the `kind` repository on the HTTPS mock.
const fn install(name: &'static str, cmdline: &'static str, kind: MirrorKind) -> TestCase {
    TestCase {
        server: false,
        https: Some(Https {
            content: nothing,
            faults: &[],
        }),
        data: Some(VIRTIO),
        online: Some(kind),
        ..case(name, cmdline)
    }
}

/// Boot 2 of `pkg-online`: undo, then the same install again, from the store,
/// with no request for a piece.
const INSTALL_AGAIN: TestCase = TestCase {
    expect: &[
        "pkg: undo switched to generation 3; big installed from the online repository as generation 4, fetched in 0 pieces",
        "pkg-online: boot 2: fs shows generation 4 current, as pkg left it",
    ],
    ..install(
        "pkg-online (boot 2)",
        "test=pkg-online again",
        MirrorKind::Signed,
    )
};

/// Boot 2 of `pkg-online-rollback`: the repository now serves a newer timestamp
/// naming an older snapshot than the one boot 1 trusted.
const ROLLBACK: TestCase = TestCase {
    expect: &[
        "pkg: the online repository's index is refused: bad version",
        "pkg-online: boot 2: fs shows generation 2 current, as pkg left it",
    ],
    ..install(
        "pkg-online-rollback (boot 2)",
        "test=pkg-online rollback",
        MirrorKind::RolledBack,
    )
};

pub(super) const CASES: &[TestCase] = &[
    // P5 task 8: `pkg` installs from the repository the HTTPS mock serves as
    // GitHub does, over TLS 1.3 on its own `NetDest`: the metadata from the Pages
    // host, the package in 192 KiB ranges from the releases host through its
    // redirect to the release-assets host, checked with kaituf against the root
    // the image carries. A name off the repository's hosts is refused by the
    // handle before a query leaves. Undo, then the install again comes from the
    // store with no fetch.
    TestCase {
        expect: &[
            "pkg: the destination refused elsewhere.kai.test, which is not the repository's",
            "pkg: big installed from the online repository as generation 2, fetched in 2 pieces",
            "pkg-online: generation 2 holds big from the online repository beside the image's notes",
            "pkg-online: pkg starts with one NetDest handle, for the repository's three hosts",
            "pkg-online: boot 1: fs shows generation 2 current, as pkg left it",
        ],
        then: Some(&INSTALL_AGAIN),
        ..install("pkg-online", "test=pkg-online", MirrorKind::Signed)
    },
    // ADR 0011's five tamper cases over the network, each refused by name,
    // switching nothing.
    TestCase {
        expect: &[
            "pkg: the online repository's big is refused: BadHash(\"big\")",
            "pkg-online: boot 1: fs shows generation 1 current, as pkg left it",
        ],
        ..install(
            "pkg-online-flipped",
            "test=pkg-online tampered",
            MirrorKind::FlippedByte,
        )
    },
    TestCase {
        expect: &[
            "pkg: the online repository's index is refused: expired: timestamp",
            "pkg-online: boot 1: fs shows generation 1 current, as pkg left it",
        ],
        ..install(
            "pkg-online-expired",
            "test=pkg-online tampered",
            MirrorKind::Expired,
        )
    },
    TestCase {
        expect: &[
            "pkg: big installed from the online repository as generation 2, fetched in 2 pieces",
            "pkg-online: boot 1: fs shows generation 2 current, as pkg left it",
        ],
        then: Some(&ROLLBACK),
        ..install("pkg-online-rollback", "test=pkg-online", MirrorKind::Newer)
    },
    TestCase {
        expect: &[
            "pkg: the online repository's index is refused: below the signature threshold",
            "pkg-online: boot 1: fs shows generation 1 current, as pkg left it",
        ],
        ..install(
            "pkg-online-unknown-key",
            "test=pkg-online tampered",
            MirrorKind::UnknownKey,
        )
    },
    TestCase {
        expect: &[
            "pkg: the online repository's big is refused: HeaderDiffers(\"big\")",
            "pkg-online: boot 1: fs shows generation 1 current, as pkg left it",
        ],
        ..install(
            "pkg-online-header",
            "test=pkg-online tampered",
            MirrorKind::HeaderDiffers,
        )
    },
    TestCase {
        then: Some(&A_FROM_NET),
        ..slot_boot("update-online", "test=slot panic=reset", INSTALL)
    },
    TestCase {
        then: Some(&A_REFUSES_FLIP),
        ..slot_boot("update-online-flipped", "test=slot panic=reset", INSTALL)
    },
    TestCase {
        then: Some(&A_REFUSES_EXPIRED),
        ..slot_boot("update-online-expired", "test=slot panic=reset", INSTALL)
    },
];

// ---- updates over the network (P5 task 10)

/// A slot boot: ends in a reset, which QEMU reports as status 0 under
/// `-no-reboot`. On the `kai-data` disk, so `updated` has `/system/trust` and
/// `/system/updates`.
const fn slot_boot(
    name: &'static str,
    cmdline: &'static str,
    expect: &'static [&'static str],
) -> TestCase {
    TestCase {
        expect,
        server: false,
        resets: true,
        slots: true,
        data: Some(VIRTIO),
        ..case(name, cmdline)
    }
}

/// A slot boot where `updated` reaches the HTTPS mock, which serves this build
/// as release 2 with `fault`.
const fn online_boot(
    name: &'static str,
    expect: &'static [&'static str],
    fault: Fault,
) -> TestCase {
    TestCase {
        https: Some(Https {
            content: nothing,
            faults: &[],
        }),
        online_release: Some(fault),
        ..slot_boot(name, "test=update updated=online panic=reset", expect)
    }
}

/// The first boot of a chain: a fresh install in R, which arms A.
const INSTALL: &[&str] = &[
    "slots: booted recovery (record missing)",
    "slots: in R with no record: installed release 1 in kai-a and kai-b as trials",
    "slots: R arms kai-a (trial)",
    "slots: next boot kai-a",
];

/// `update-online` boot 3: B boots release 2, and the same repository offered
/// again is refused, now from the root `/system/trust` kept.
const B_FROM_NET: TestCase = online_boot(
    "update-online (3: kai-b boots release 2)",
    &[
        "slots: booted kai-b (record read)",
        "slots: up in kai-b (trial, 2 tries left)",
        "updated: reconciled: OnTrial",
        "updated: the trusted root is /system/trust's",
        "updated: FAIL refused: release 2 is not newer than 2 (anti-rollback)",
        "updated: restarting into the running slot",
    ],
    Fault::None,
);

/// `update-online` boot 2: `updated` in A reads the repository's metadata over
/// TLS, streams release 2 into B in checked 2 MiB chunks, and restarts into it.
const A_FROM_NET: TestCase = TestCase {
    then: Some(&B_FROM_NET),
    ..online_boot(
        "update-online (2: updated writes kai-b over the network)",
        &[
            "updated: the trusted root is the slot's own",
            "updated: the online repository verifies: slot.img is release 2, ",
            "updated: took update snapshot ",
            "updated: wrote release 2 into B over TLS and checked it: 0 chunks kept, ",
            "updated: the verified metadata is in /system/trust",
            "update: restarting into kai-b",
        ],
        Fault::None,
    )
};

/// `update-online-flipped` boot 3: A again, and B, refused, cannot boot.
const A_AFTER_FLIP: TestCase = slot_boot(
    "update-online-flipped (3: still kai-a)",
    "test=slot panic=reset",
    &[
        "slots: booted kai-a (record read)",
        "slots: up in kai-a (trial, 1 tries left)",
        "slots: kai-b cannot boot (Updating)",
    ],
);

/// `update-online-flipped` boot 2: one byte of the image the mock serves is not
/// the one the signed chunk list names, so its chunk is refused and not written.
const A_REFUSES_FLIP: TestCase = TestCase {
    then: Some(&A_AFTER_FLIP),
    ..online_boot(
        "update-online-flipped (2: updated refuses a chunk)",
        &[
            "updated: the online repository verifies: slot.img is release 2, ",
            "is not the signed one; the slot stays unbootable",
            "updated: restarting into the running slot",
        ],
        Fault::FlippedByte,
    )
};

/// `update-online-expired` boot 2: the repository's timestamp lapsed, so its
/// metadata is refused by name and nothing is written.
const A_REFUSES_EXPIRED: TestCase = online_boot(
    "update-online-expired (2: updated refuses the metadata)",
    &[
        "updated: FAIL refused: the repository was refused: expired: timestamp",
        "updated: restarting into the running slot",
    ],
    Fault::Expired,
);
