//! The hardware watchdog's cases (ADR 0008 §5): trials that hang with
//! interrupts off, which only the i6300esb resets, R's count of each lost
//! boot, and a committed boot that outlives its disarm.
//!
//! Only the boots meant to hang, and the one that waits out its disarm, give
//! the watchdog 5 s: a healthy boot armed that briefly resets on a 4 s vCPU
//! stall, so the rest keep the 30 s default (#1632's review).

use super::boot;
use crate::cases::TestCase;

/// `watchdog-hang` (ADR 0008 §5): B, staged, hangs with interrupts off before
/// its checks on each of its three tries. Only the i6300esb resets it, so R
/// finds a boot that never came up, spends a try for it, and after the third
/// moves to A. The watchdog gives a trial 5 s here.
const fn hangs(name: &'static str) -> TestCase {
    TestCase {
        watchdog: true,
        ..boot(
            name,
            "test=slot slot-do=hang watchdog_s=5 panic=reset",
            &[
                "slots: booted kai-b",
                "watchdog: armed for 5 s",
                "slots: slot-do=hang: interrupts off on CPU ",
            ],
        )
    }
}

/// Boot 10 (#1632's review): committed kai-a, whose lever disarms the 5 s
/// watchdog bind armed, stays up 12 s. A disarm that left the device counting
/// would reset it, and a watchdog reset fails a boot not meant to hang.
const WD_A_AGAIN: TestCase = boot(
    "watchdog-hang (10: back on kai-a, up 12 s past its disarm)",
    "test=slot slot-do=wait watchdog_s=5 panic=reset",
    &[
        "slots: booted kai-a (record read)",
        "watchdog: armed for 5 s",
        "watchdog: disarmed",
        "slots: up in kai-a (committed, 0 tries left)",
        "slots: kai-b cannot boot (Unbootable)",
        "slots: slot-do=wait: up 12 s past the lever, with no reset",
        "slots: next boot kai-a",
    ],
);
const WD_R_GIVES_UP: TestCase = TestCase {
    then: Some(&WD_A_AGAIN),
    ..boot(
        "watchdog-hang (9: R moves to kai-a)",
        "test=slot panic=reset",
        &[
            "slots: booted recovery (record read)",
            "slots: R arms kai-a (committed)",
        ],
    )
};
const WD_HANG_3: TestCase = TestCase {
    then: Some(&WD_R_GIVES_UP),
    ..hangs("watchdog-hang (8: kai-b hangs a third time)")
};
const WD_R_RETRIES_2: TestCase = TestCase {
    then: Some(&WD_HANG_3),
    ..boot(
        "watchdog-hang (7: R tries kai-b again)",
        "test=slot panic=reset",
        &[
            "slots: booted recovery (record read)",
            "slots: R arms kai-b (trial)",
        ],
    )
};
const WD_HANG_2: TestCase = TestCase {
    then: Some(&WD_R_RETRIES_2),
    ..hangs("watchdog-hang (6: kai-b hangs again)")
};
const WD_R_RETRIES: TestCase = TestCase {
    then: Some(&WD_HANG_2),
    ..boot(
        "watchdog-hang (5: R tries kai-b again)",
        "test=slot panic=reset",
        &[
            "slots: booted recovery (record read)",
            "slots: R arms kai-b (trial)",
        ],
    )
};
const WD_HANG: TestCase = TestCase {
    then: Some(&WD_R_RETRIES),
    ..hangs("watchdog-hang (4: kai-b hangs before its checks)")
};
const WD_STAGES_B: TestCase = TestCase {
    then: Some(&WD_HANG),
    ..boot(
        "watchdog-hang (3: kai-a stages kai-b)",
        "test=slot slot-do=stage slot-seq=2 panic=reset",
        &[
            "slots: up in kai-a (committed, 0 tries left)",
            "slots: staged kai-b as release 2; restarting into it",
            "slots: next boot kai-b",
        ],
    )
};
pub(super) const WD_COMMITS: TestCase = TestCase {
    then: Some(&WD_STAGES_B),
    ..boot(
        "watchdog-hang (2: kai-a commits)",
        "test=slot slot-do=commit panic=reset",
        &[
            "slots: booted kai-a (record read)",
            "watchdog: armed for 30 s",
            "slots: committed kai-a",
            "slots: next boot kai-a",
        ],
    )
};

/// `watchdog-hang-cpu1` (#1571's review): A's trial stops CPU 1 with
/// interrupts off and leaves CPU 0, which pets, ticking, as its line a second
/// later shows. Only the rule that every online CPU ticks between pets resets
/// it, and R spends a try for the boot that never came up.
const HC_R_RETRIES: TestCase = boot(
    "watchdog-hang-cpu1 (3: R tries kai-a again)",
    "test=slot panic=reset",
    &[
        "slots: booted recovery (record read)",
        "slots: R arms kai-a (trial)",
    ],
);
pub(super) const HC_HANGS: TestCase = TestCase {
    then: Some(&HC_R_RETRIES),
    watchdog: true,
    ..boot(
        "watchdog-hang-cpu1 (2: kai-a stops CPU 1)",
        "test=slot slot-do=hang-cpu1 watchdog_s=5 panic=reset",
        &[
            "slots: booted kai-a (record read)",
            "watchdog: armed for 5 s",
            "slots: slot-do=hang: interrupts off on CPU 1 for good",
            "slots: slot-do=hang-cpu1: CPU 0 still ticks 1 s after CPU 1 stopped",
        ],
    )
};

/// `watchdog-hang-up` (#1571's review): B, staged, hangs after *up*, before
/// probation starts. While tries are left the one-shot names B again, so each
/// watchdog reset boots it and its *up* spends the next try; after the last,
/// R boots next, marks B Unbootable and moves to A, whose boot reads it back.
const HU_A_AGAIN: TestCase = boot(
    "watchdog-hang-up (8: back on kai-a)",
    "test=slot panic=reset",
    &[
        "slots: booted kai-a (record read)",
        "slots: up in kai-a (committed, 0 tries left)",
        "slots: kai-b cannot boot (Unbootable)",
        "slots: next boot kai-a",
    ],
);
const HU_R_GIVES_UP: TestCase = TestCase {
    then: Some(&HU_A_AGAIN),
    ..boot(
        "watchdog-hang-up (7: R moves to kai-a)",
        "test=slot panic=reset",
        &[
            "slots: booted recovery (record read)",
            "slots: R arms kai-a (committed)",
        ],
    )
};
const fn hangs_up(name: &'static str, expect: &'static [&'static str]) -> TestCase {
    TestCase {
        watchdog: true,
        ..boot(
            name,
            "test=slot slot-do=hang-up watchdog_s=5 panic=reset",
            expect,
        )
    }
}
const HU_HANG_3: TestCase = TestCase {
    then: Some(&HU_R_GIVES_UP),
    ..hangs_up(
        "watchdog-hang-up (6: kai-b's last try hangs after up)",
        &[
            "slots: booted kai-b (record read)",
            "watchdog: armed for 5 s",
            "slots: up in kai-b (trial, no tries left, 0 tries left)",
            "slots: slot-do=hang: interrupts off on CPU ",
        ],
    )
};
const HU_HANG_2: TestCase = TestCase {
    then: Some(&HU_HANG_3),
    ..hangs_up(
        "watchdog-hang-up (5: kai-b, booted again, hangs after up)",
        &[
            "slots: booted kai-b (record read)",
            "watchdog: armed for 5 s",
            "slots: up in kai-b (trial, 1 tries left)",
            "slots: slot-do=hang: interrupts off on CPU ",
        ],
    )
};
const HU_HANG: TestCase = TestCase {
    then: Some(&HU_HANG_2),
    ..hangs_up(
        "watchdog-hang-up (4: kai-b hangs after up)",
        &[
            "slots: booted kai-b (record read)",
            "watchdog: armed for 5 s",
            "slots: up in kai-b (trial, 2 tries left)",
            "slots: slot-do=hang: interrupts off on CPU ",
        ],
    )
};
const HU_STAGES_B: TestCase = TestCase {
    then: Some(&HU_HANG),
    ..boot(
        "watchdog-hang-up (3: kai-a stages kai-b)",
        "test=slot slot-do=stage slot-seq=2 panic=reset",
        &[
            "slots: up in kai-a (committed, 0 tries left)",
            "watchdog: disarmed",
            "slots: next boot kai-b",
        ],
    )
};
pub(super) const HU_COMMITS: TestCase = TestCase {
    then: Some(&HU_STAGES_B),
    ..boot(
        "watchdog-hang-up (2: kai-a commits)",
        "test=slot slot-do=commit panic=reset",
        &[
            "slots: booted kai-a (record read)",
            "watchdog: armed for 30 s",
            "slots: committed kai-a",
            "slots: next boot kai-a",
        ],
    )
};
