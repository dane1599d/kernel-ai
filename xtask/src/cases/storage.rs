//! S, storage (ADR 0004): spike S04-3's raw half, block writes and flushes
//! replayed from QEMU's blklogwrites log, on the never-merged branch
//! spike/blk-flush.

use super::{TestCase, case};
use crate::blklog::Expect;

pub(super) const CASES: &[TestCase] = &[
    TestCase {
        expect: &["blk-flush: 200 commits durable"],
        server: false,
        data_log: Some(Expect::Clean),
        ..case("blk-flush", "test=blk-flush")
    },
    // The harness must see a missing flush: the guest skips the first flush of
    // every commit, and the replay must find violations.
    TestCase {
        expect: &["blk-flush: 200 commits durable"],
        server: false,
        data_log: Some(Expect::Caught),
        ..case("blk-flush-bug", "test=blk-flush blkbug=noflush")
    },
];
