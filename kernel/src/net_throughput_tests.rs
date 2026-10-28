//! `test=net-throughput` (spike P5-S2 `net-throughput`, docs/plans/p5-networking.md
//! §5, not for merge): download speed from xtask's blob server (`blob=`) over
//! QEMU's user network, with receive buffers of 64 KiB, 1 MiB and 4 MiB, under
//! today's polled driver (a 2 ms gap between polls) and polling again at once.
//! xtask adds the round trip (`rtt_ms=`, a QEMU filter that holds the frames to
//! the guest). Figures are reported against the bar (at least 30 MB/s at 20 ms),
//! not asserted; the case passes when every download got data.

use crate::net::{self, DownloadPlan};
use crate::{hpet, log, threads};
use alloc::format;

const RX_BUFFERS_KIB: [usize; 3] = [64, 1024, 4096];
/// Today's poll gap (`MAX_POLL_GAP_MS`), and none.
const POLL_GAPS_MS: [u64; 2] = [2, 0];
const RUN_MS: u64 = 6_000;
const MAX_BYTES: u64 = 1 << 30;
const ADDRESS_WAIT_MS: u64 = 15_000;

fn word(name: &str) -> Option<&'static str> {
    crate::cmdline()
        .split_whitespace()
        .find_map(|w| w.strip_prefix(name))
}

pub fn check() -> Result<(), &'static str> {
    let server = word("blob=")
        .and_then(net::parse_endpoint)
        .ok_or("net-throughput: no blob= server on the command line")?;
    let rtt = word("rtt_ms=").unwrap_or("0");
    let until = hpet::nanos() + ADDRESS_WAIT_MS * 1_000_000;
    while net::status().and_then(|s| s.address).is_none() {
        if hpet::nanos() > until {
            return Err("net-throughput: no address");
        }
        threads::sleep_ms(10);
    }
    let request =
        format!("GET /blob/{MAX_BYTES} HTTP/1.1\r\nHost: blob\r\nConnection: close\r\n\r\n");
    for gap in POLL_GAPS_MS {
        for kib in RX_BUFFERS_KIB {
            let plan = DownloadPlan {
                rx_buffer: kib * 1024,
                poll_gap_ms: gap,
                max_bytes: MAX_BYTES,
                run_ms: RUN_MS,
            };
            let got = net::download(server, request.as_bytes(), &plan).map_err(|error| {
                log!("net-throughput: {kib} KiB, gap {gap} ms: {error}");
                "net-throughput: a download failed"
            })?;
            log!(
                "net-throughput: added RTT {rtt} ms, receive buffer {kib} KiB, poll gap {gap} ms: {} MB/s ({} MiB in {} ms; bar 30 MB/s at 20 ms)",
                got.bytes * 1000 / got.ns.max(1),
                got.bytes >> 20,
                got.ns / 1_000_000,
            );
        }
    }
    log!("net-throughput: done");
    Ok(())
}
