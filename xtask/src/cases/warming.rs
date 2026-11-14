//! Founder test 2: "Kai is getting ready" while llm fills its prompt slots,
//! after a rebuild that changed the prompt (`user/src/bin/shell/warmth.rs`).

use super::{TestCase, case};

pub(super) const CASES: &[TestCase] = &[
    // llm says it is not warm for its first 6 s (`llm.cold_ms`, a test word),
    // as a local model filling its slots would. The shell tells `desktop`,
    // whose panel and top bar then say "Kai is getting ready", and then that
    // it is warm once llm says so. The desk is the default boot's, with the
    // mock model.
    TestCase {
        expect: &[
            "desktop: the model is warming up",
            "desktop: the model is warm",
        ],
        stops: true,
        ..case("desk-warming", "test=desk-boot llm.cold_ms=6000")
    },
];
