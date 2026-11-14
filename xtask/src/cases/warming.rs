//! Founder test 2: "Kai is getting ready" while llm fills its prompt slots,
//! after a rebuild that changed the prompt (`user/src/bin/shell/warmth.rs`).

use super::{TestCase, case};
use crate::blklog::Data;

/// `kai-data` on a virtio disk, as `desk-default` boots it.
const VIRTIO: Data = Data {
    nvme: false,
    log: None,
    gpt: true,
};

pub(super) const CASES: &[TestCase] = &[
    // llm says it is not warm for its first 20 s (`llm.cold_ms`, a test word),
    // as a local model filling its slots would. The shell tells `desktop`,
    // whose panel and top bar then say "Kai is getting ready", and then that
    // it is ready once llm says so. The system is the default boot's, as
    // `desk-default` boots it, with the mock model: there the shell has its
    // route to `desktop` (`kabi::plan::SHELL_DESK`). `test=desk-warming` only
    // keeps the boot up past the warm-up; QEMU stops once both lines are in.
    // The desktop and the shell come up within a second of llm, under TCG too,
    // so 20 s leaves room to see llm still say no.
    TestCase {
        server: true,
        data: Some(VIRTIO),
        memory_mib: Some(2048),
        expect: &[
            "desktop: the model is getting ready",
            "desktop: the model is ready",
        ],
        stops: true,
        ..case("desk-warming", "test=desk-warming llm.cold_ms=20000")
    },
];
