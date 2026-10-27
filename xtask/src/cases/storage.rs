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
