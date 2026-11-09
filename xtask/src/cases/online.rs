//! The OS's own TLS clients (P5 tasks 8 and 10): `pkg` and `updated` reaching
//! their repositories over the network, on a `NetDest` of their own, against
//! xtask's HTTPS mock (`mock_https`) as GitHub serves them, through the harness
//! of P5 task 12 (`cases::https`): the mock's hosts resolve through the mock DNS
//! zone, QEMU forwards port 443 there, and its test CA is the bundle's
//! `netroots/1.der`. No case reaches the internet.

use super::{Https, TestCase, case};
use crate::blklog::Data;
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
];
