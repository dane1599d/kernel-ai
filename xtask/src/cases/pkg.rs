//! P, packages (ADR 0011): the `pkg` service and its store on `kai-data`.

use super::{TestCase, case};
use crate::blklog::Data;

const VIRTIO: Data = Data {
    nvme: false,
    log: None,
    gpt: true,
};

/// Boot 3 of `pkg-store`: the undo made in boot 2 is still current.
const BOOT_3: TestCase = TestCase {
    expect: &[
        "pkg: boot 3: generation 3 is current, without hello: the undo survived the reboot",
        "pkg-store: boot 3: fs shows generation 3 current, as pkg left it",
    ],
    server: false,
    data: Some(VIRTIO),
    ..case("pkg-store (boot 3)", "test=pkg-store check")
};

/// Boot 2 of `pkg-store`: the install survived the reboot; then undo.
const BOOT_2: TestCase = TestCase {
    expect: &[
        "pkg: boot 2: generation 2 was current with hello's object whole; undo switched to generation 3",
        "pkg-store: boot 2: fs shows generation 3 current, as pkg left it",
    ],
    server: false,
    data: Some(VIRTIO),
    then: Some(&BOOT_3),
    ..case("pkg-store (boot 2)", "test=pkg-store undo")
};

/// Boot 2 of `pkg-image`: generation 1 stays, seeded once.
const IMAGE_AGAIN: TestCase = TestCase {
    expect: &[
        "pkg: the image's repository verified, 1 kept and 0 refused; generation 1 stays current: notes 1.0.0",
        "pkg-image: generation 1 is notes 1.0.0, from the image's signed repository",
        "pkg-image: boot 2: fs shows generation 1 current, as pkg left it",
    ],
    server: false,
    data: Some(VIRTIO),
    ..case("pkg-image (boot 2)", "test=pkg-image again")
};

pub(super) const CASES: &[TestCase] = &[
    // P4 task 1: `pkg` reads the image's signed repository from the boot bundle
    // with BOOT_READ, which its plan line's `boot=pkgrepo` grants beneath
    // `pkgrepo/` only, checks it with kaituf, and makes it generation 1.
    TestCase {
        expect: &[
            "pkg: BOOT_READ refuses every path outside pkgrepo/",
            "pkg: the image's repository verified, 1 kept and 0 refused; generation 1 is its packages: notes 1.0.0",
            "pkg-image: generation 1 is notes 1.0.0, from the image's signed repository",
            "pkg-image: boot 1: fs shows generation 1 current, as pkg left it",
        ],
        server: false,
        data: Some(VIRTIO),
        then: Some(&IMAGE_AGAIN),
        ..case("pkg-image", "test=pkg-image")
    },
    // The same with one byte of notes's target flipped in the bundle, whose own
    // digests are recomputed: only the signed metadata can tell, and notes is
    // refused by name while the rest boots.
    TestCase {
        expect: &[
            "pkg: the image's notes-1.0.0.kpkg is refused: length or hash mismatch",
            "pkg: the image's repository verified, 0 kept and 1 refused; generation 1 is its packages: no packages",
            "pkg-image: generation 1 holds no package: the tampered notes is not in it",
        ],
        server: false,
        data: Some(VIRTIO),
        tampered: Some("notes"),
        ..case("pkg-image-tampered", "test=pkg-image tampered")
    },
    // P2 task 13, the first slice of P4 task 1: kaipkg's store in `pkg` over
    // `fs`, holding only the `/system/pkg` its plan line's `dir=pkg` grants. An
    // install survives a reboot, and undo puts the previous generation back
    // after a reboot (P2 plan §1).
    TestCase {
        expect: &[
            "fs: up; kai-data has ",
            "pkg: boot 1: hello 1.0.0 installed as generation 2, fetched in 2 pieces",
            "pkg-store: pkg holds no capability bits and starts with only /system/pkg's Dir",
            "pkg-store: boot 1: fs shows generation 2 current, as pkg left it",
        ],
        server: false,
        data: Some(VIRTIO),
        then: Some(&BOOT_2),
        ..case("pkg-store", "test=pkg-store")
    },
];
