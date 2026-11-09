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
            "[netcheck-nocap] resolve kai.test refused: not under this handle's hosts",
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
    // P5 task 1: a NetDest handle resolves only names under its hosts, a
    // connect counts only for what it resolved or names, and it only shrinks.
    TestCase {
        dns: true,
        server: false,
        expect: &[
            "[netdest-a] resolved alias.kai.test to 10.0.2.99",
            "net-dest: alias.kai.test resolved on the handle, and only its address and the literal range may be connected to, on port 8000",
            "[netdest-b] resolve leak.attacker.example refused: not under this handle's hosts",
            "net-dest: a name under none of its hosts was refused, and no query left",
            "[netdest-c] resolved alias.kai.test to 10.0.2.99",
            "[netdest-d] resolve kai.test refused: not under this handle's hosts",
            "[netdest-e] narrowing to proto:tcp,ports:any,hosts:kai.test refused: it would allow more",
            "net-dest: a narrower handle allowed only its own host, and a wider one was refused",
            "[netdest-f] resolve kai.test refused: not under this handle's hosts",
            "net-dest: the retired RESOLVE refused even a holder of a NetDest",
            "[netdest-g] http without the LLM's own handle refused, with 1 other NetDest held",
            "net-dest: HTTP to the LLM needs the LLM's own handle, not any NetDest",
        ],
        ..case("net-dest", "test=net-dest")
    },
    // P5 task 2: TCP streams on the NetDest to xtask's echo server: echo,
    // the 8 KiB and 16-stream limits, a port off the handle, and a kill.
    TestCase {
        echo: true,
        server: false,
        expect: &[
            "[netstream-a] a stream echoed 8192 and 20 bytes, refused 8193 in one send, and closed",
            "[netstream-a] a bulk stream echoed 8192 and 20 bytes, refused 8193 in one send, and closed",
            "is not on the handle: refused",
            "[netstream-a] 16 streams open; the 17th was Busy",
            "[netstream-a] after closing them, by NET_CLOSE or by the handle, 16 more opened",
            "[netstream-a] a third bulk stream of one process was Busy",
            "net-stream: streams echoed through the handle, kept to its port and to 16, and closed",
            // Not "[netstream-b] holding 3 streams": the kernel kills b once
            // its own count shows 3 open, which can be before b's line reaches
            // the console (run 41942359956). That count is the proof b held them.
            "net-stream: a process killed with 3 streams open left none",
        ],
        ..case("net-stream", "test=net-stream")
    },
];
