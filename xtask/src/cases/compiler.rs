//! T, the terminal compiler (ADR 0012): spike S12-1's boot case, on the never-merged
//! branch spike/cc-wasm. Needs `cargo xtask ccwasm` first, for `ccspike`'s cc.wasm.

use super::{TestCase, case};
use std::time::Duration;

pub(super) const CASES: &[TestCase] = &[
    TestCase {
        expect: &["ccspike: all checks passed"],
        server: false,
        min_timeout: Some(Duration::from_secs(1500)),
        ..case("ccspike", "test=ccspike")
    },
    // The same with one compile per entry, for TCG, whose figures are reported only.
    TestCase {
        expect: &["ccspike: all checks passed"],
        server: false,
        min_timeout: Some(Duration::from_secs(1500)),
        ..case("ccspike-quick", "test=ccspike repeats=1")
    },
];
