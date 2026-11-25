//! P5-S2's bar on main's driver (`net-throughput`, lane S-io): booted only by
//! name, by the throughput workflow, which serves `/blob` on 10.0.2.2:8765 over
//! a TAP device (`XTASK_NETDEV`) and adds the round trip with netem.

use super::{TestCase, case};
use std::time::Duration;

pub const NET_THROUGHPUT: TestCase = TestCase {
    expect: &["net-throughput: ", " MB/s, first byte "],
    server: false,
    min_timeout: Some(Duration::from_secs(600)),
    ..case(
        "net-throughput",
        "test=net-throughput ip=static blob=10.0.2.2:8765",
    )
};
