//! U, updates and the trust root: `kaituf`, `bootslot`, `updated`.

use super::{TestCase, case};

pub(super) const CASES: &[TestCase] = &[
    // Spikes S08-3 and S11-1: P-256 and Ed25519 verification, a 1,000-target
    // `packages` role and SHA-256 in soft-float ring 3, reported in the log.
    TestCase {
        expect: &["tuf-bench: done"],
        server: false,
        ..case("tuf-bench", "test=tuf-bench")
    },
];
