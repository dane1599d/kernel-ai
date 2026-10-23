//! N, the network: DHCP, DNS, ping and the static fallback.

use super::{TestCase, case};

pub(super) const CASES: &[TestCase] = &[
    // P5: DHCP from slirp, ping to its gateway, the resolver against the mock DNS
    // server, RESOLVE from ring 3, and renewal of a lease capped at 4 s.
    TestCase {
        dns: true,
        server: false,
        expect: &[
            "net: DHCP lease 10.0.2.15/24 via 10.0.2.2 from 10.0.2.2, DNS 10.0.2.3, lease 4s",
            "net-setup: 3 of 3 pings to the gateway answered",
            "net-setup: kai.test resolved to 10.0.2.99 (TTL 300 s), then from the cache",
            "net-setup: silent.kai.test timed out after 1500 ms as asked",
            "[netcheck] resolved kai.test to 10.0.2.99",
            "[netcheck-nocap] resolve kai.test refused: no NET capability",
            "net-setup: the 4 s lease was renewed: still leased after 10 s, none lost",
            "network: 10.0.2.15/24 via 10.0.2.2, from the DHCP server at 10.0.2.2",
            "lease: 4s, granted ",
            "resolve: alias.kai.test is 10.0.2.99 (cached, ",
            "ping: reply from 10.0.2.2, seq 0, ",
            "ping: 10.0.2.2: 1 sent, 1 received, 0% lost",
        ],
        ..case("net-setup", "test=net-setup dhcp_max_lease_s=4")
    },
    // P5: DHCP sent where nobody serves it; the fallback comes after 1.5 s.
    TestCase {
        server: false,
        expect: &[
            "net: no DHCP answer in 1500 ms; using 10.0.2.15/24 via 10.0.2.2 while asking on",
            "net-fallback: 3 of 3 pings to the gateway answered",
            "network: 10.0.2.15/24 via 10.0.2.2, the fallback",
        ],
        ..case(
            "net-fallback",
            "test=net-fallback dhcp_port=6767 dhcp_wait_ms=1500",
        )
    },
    TestCase {
        dns: true,
        server: false,
        expect: &[
            "static 10.0.2.15/24 via 10.0.2.2",
            "net-static: 3 of 3 pings to the gateway answered",
        ],
        ..case("net-static", "test=net-static ip=static")
    },
];
