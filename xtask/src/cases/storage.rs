//! S, storage (ADR 0004): spike S04-1's in-target checksum figure, on the
//! never-merged branch spike/kaifs.

use super::{TestCase, case};

pub(super) const CASES: &[TestCase] = &[TestCase {
    expect: &["hashbench: done"],
    server: false,
    ..case("hashbench", "test=hashbench")
}];
