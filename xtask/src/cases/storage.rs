//! S, storage (ADR 0004, P2): block writes and flushes on the `kai-data` disk,
//! and S04-3's raw case, replayed from QEMU's blklogwrites log.

use super::{TestCase, case};
use crate::blklog::{Data, Expect};

const VIRTIO: Data = Data {
    nvme: false,
    log: None,
    gpt: true,
};
const NVME: Data = Data {
    nvme: true,
    ..VIRTIO
};
/// S04-3's commits fill a raw disk.
const RAW: Data = Data {
    gpt: false,
    ..VIRTIO
};

/// Boot 2 of `blk-write`: what boot 1 wrote and flushed reads back.
const REREAD: TestCase = TestCase {
    expect: &["blk-write: boot 1's writes read back"],
    server: false,
    data: Some(VIRTIO),
    ..case("blk-write (boot 2)", "test=blk-write reread")
};
const REREAD_NVME: TestCase = TestCase {
    data: Some(NVME),
    ..REREAD
};

/// Boot 2 of `fs-reboot`: what boot 1 wrote through `fs` reads back.
const FS_REBOOT_2: TestCase = TestCase {
    expect: &[
        "filecheck: boot 2 read back what boot 1 wrote, byte for byte",
        "files: boot 2: filecheck's step through its Dir handle passed",
    ],
    server: false,
    data: Some(VIRTIO),
    ..case("fs-reboot (boot 2)", "test=files reread")
};

pub(super) const CASES: &[TestCase] = &[
    TestCase {
        expect: &[
            "block: kai-data is ",
            "block: kai-cache is ",
            "blk-write: kai-data over blk takes writes and a flush",
        ],
        server: false,
        data: Some(VIRTIO),
        then: Some(&REREAD),
        ..case("blk-write", "test=blk-write")
    },
    TestCase {
        expect: &[
            "blk-write: kai-data over nvme takes writes and a flush",
            ", volatile write cache, AWUPF",
        ],
        server: false,
        data: Some(NVME),
        then: Some(&REREAD_NVME),
        ..case("blk-write-nvme", "test=blk-write")
    },
    // P2 task 3: `fs` alone holds a Partition, for kai-data only; S04-2 re-run on
    // the real calls.
    TestCase {
        expect: &[
            "fs: up; kai-data has ",
            "part: fs alone holds a Partition, for kai-data only, and its checks pass",
            "part: S04-2 on the real calls: ",
        ],
        server: false,
        data: Some(VIRTIO),
        ..case("part", "test=part")
    },
    TestCase {
        expect: &[
            "part: fs alone holds a Partition, for kai-data only, and its checks pass",
            "part: S04-2 on the real calls: ",
        ],
        server: false,
        data: Some(NVME),
        ..case("part-nvme", "test=part")
    },
    // P2 task 8: files through a `Dir` handle survive a reboot, and `fs` killed
    // mid-action restarts with nothing half done and its handles working.
    TestCase {
        expect: &[
            "fs: up; kai-data has ",
            "filecheck: boot 1 wrote notes/plan.txt and a 20 KiB file through its Dir handle",
            "files: boot 1: filecheck's step through its Dir handle passed",
            "files: ENDPOINT_MINT, the FILES ceiling and SYSTEM_STATE refuse what they must",
        ],
        server: false,
        data: Some(VIRTIO),
        then: Some(&FS_REBOOT_2),
        ..case("fs-reboot", "test=files")
    },
    TestCase {
        expect: &[
            "filecheck: the File handle opened before fs restarted reads what was committed",
            "files: fs restarted mid-action: nothing half-done is on disk, and an open File handle works again",
        ],
        server: false,
        data: Some(VIRTIO),
        ..case("fs-restart", "test=files-restart")
    },
    // S04-3 raw: 200 commits shaped like kaifs's, every flush point replayed.
    TestCase {
        expect: &["blk-flush: 200 commits durable"],
        server: false,
        data: Some(Data {
            log: Some(Expect::Clean),
            ..RAW
        }),
        ..case("blk-flush", "test=blk-flush")
    },
    // The harness must see a missing flush: the guest skips the first flush of
    // every commit, and the replay must find violations.
    TestCase {
        expect: &["blk-flush: 200 commits durable"],
        server: false,
        data: Some(Data {
            log: Some(Expect::Caught),
            ..RAW
        }),
        ..case("blk-flush-bug", "test=blk-flush blkbug=noflush")
    },
];
