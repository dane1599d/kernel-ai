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

/// Spike S07-4 (`spike/netd-ring3` only): 1 GiB through the kernel's stack to
/// a ring-3 client, and the same through `netd` in ring 3. The workflow serves
/// it with `cargo xtask netd-blob` over the TAP device.
pub const NETD_KERNEL: TestCase = TestCase {
    expect: &["netd-spike: mode=kernel "],
    server: false,
    min_timeout: Some(Duration::from_secs(600)),
    ..case(
        "netd-kernel",
        "test=netd-spike ip=static blob=10.0.2.2:8765 bytes=1073741824",
    )
};

pub const NETD_RING3: TestCase = TestCase {
    expect: &["netd-spike: mode=ring3 "],
    server: false,
    min_timeout: Some(Duration::from_secs(600)),
    ..case(
        "netd-ring3",
        "test=netd-spike netd=ring3 ip=static blob=10.0.2.2:8765 bytes=1073741824",
    )
};
