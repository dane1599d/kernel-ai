//! S, storage (ADR 0004, P2): block writes and flushes on the `kai-data` disk,
//! and S04-3's raw case, replayed from QEMU's blklogwrites log.

use super::{TestCase, case};
use crate::blklog::{Data, Expect};

const VIRTIO: Data = Data {
    nvme: false,
    log: None,
};

pub(super) const CASES: &[TestCase] = &[
    TestCase {
        expect: &["blk-write: kai-data over blk takes writes and a flush"],
        server: false,
        data: Some(VIRTIO),
        ..case("blk-write", "test=blk-write")
    },
    TestCase {
        expect: &[
            "blk-write: kai-data over nvme takes writes and a flush",
            "volatile write cache",
        ],
        server: false,
        data: Some(Data {
            nvme: true,
            ..VIRTIO
        }),
        ..case("blk-write-nvme", "test=blk-write")
    },
    // S04-3 raw: 200 commits shaped like kaifs's, every flush point replayed.
    TestCase {
        expect: &["blk-flush: 200 commits durable"],
        server: false,
        data: Some(Data {
            log: Some(Expect::Clean),
            ..VIRTIO
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
            ..VIRTIO
        }),
        ..case("blk-flush-bug", "test=blk-flush blkbug=noflush")
    },
];
