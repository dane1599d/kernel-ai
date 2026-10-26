//! S, storage (ADR 0004): spike S04-2, `fs` over copied IPC, on the never-merged
//! branch spike/fs-ipc.

use super::{TestCase, case};

pub(super) const CASES: &[TestCase] = &[TestCase {
    expect: &["fs-ipc: done", "fs-stub: answered"],
    server: false,
    memory_mib: Some(1024),
    ..case("fs-ipc", "test=fs-ipc")
}];
