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

pub(super) const CASES: &[TestCase] = &[
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
