//! `test=net-throughput`, spike P5-S2: downloads from a plain HTTP server on the
//! host (10.0.2.2:8765, which the spike's workflow starts) at 0, 20 and 80 ms of
//! added round trip, with receive buffers of 64 KiB, 1 MiB and 4 MiB, under the
//! polled driver: its 2 ms poll gap as the LLM client uses it, and a busy poll
//! that only yields between polls. Each is logged; the spike's PR reads them.
//! The bar, at least 30 MB/s at 20 ms, is judged there, not here.

use crate::net::bulk::{self, Plan};
use crate::{log, net, threads};
use core::net::Ipv4Addr;

const SERVER: (Ipv4Addr, u16) = (Ipv4Addr::new(10, 0, 2, 2), 8765);
const REQUEST: &[u8] = b"GET /blob HTTP/1.0\r\nHost: 10.0.2.2\r\n\r\n";
const RTTS_MS: [u64; 3] = [0, 20, 80];
const BUFFERS: [usize; 3] = [64 * 1024, 1024 * 1024, 4 * 1024 * 1024];
const GAPS_MS: [u64; 2] = [2, 0];
/// The blob is 256 MiB; a run stops at this many bytes or after the window.
const LIMIT: u64 = 128 * 1024 * 1024;
const WINDOW_MS: u64 = 6_000;
const TIMEOUT_MS: u64 = 20_000;
/// Time for frames still held to go once the delay is off.
const SETTLE_MS: u64 = 300;

pub fn check() -> Result<(), &'static str> {
    let mut failed = 0;
    for rtt in RTTS_MS {
        for rx_buffer in BUFFERS {
            for gap_ms in GAPS_MS {
                net::set_tx_delay_ms(rtt);
                let plan = Plan {
                    server: SERVER,
                    request: REQUEST,
                    rx_buffer,
                    gap_ms,
                    limit: LIMIT,
                    window_ms: WINDOW_MS,
                    timeout_ms: TIMEOUT_MS,
                };
                let moved = bulk::download(&plan);
                net::set_tx_delay_ms(0);
                threads::sleep_ms(SETTLE_MS);
                let kib = rx_buffer / 1024;
                match moved {
                    Ok(m) => {
                        let ms = (m.elapsed_ns / 1_000_000).max(1);
                        // MB/s to a tenth: bytes per ms is kB/s.
                        let tenths = m.bytes * 10 / ms / 1000;
                        log!(
                            "net-throughput: rtt {rtt} ms, rx {kib} KiB, gap {gap_ms} ms: {} bytes in {ms} ms = {}.{} MB/s, first byte {} ms, {} polls",
                            m.bytes,
                            tenths / 10,
                            tenths % 10,
                            m.first_byte_ns / 1_000_000,
                            m.polls
                        );
                    }
                    Err(why) => {
                        failed += 1;
                        log!(
                            "net-throughput: rtt {rtt} ms, rx {kib} KiB, gap {gap_ms} ms: failed: {why}"
                        );
                    }
                }
            }
        }
    }
    if failed != 0 {
        return Err("net-throughput: a download failed");
    }
    log!("net-throughput: 18 downloads measured");
    Ok(())
}
