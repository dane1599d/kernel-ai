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
const RTTS_MS: [u64; 2] = [0, 20];
const BUFFERS: [usize; 2] = [1024 * 1024, 4 * 1024 * 1024];
const GAPS_MS: [u64; 2] = [0, 2];

/// The blob is 256 MiB; a run stops at this many bytes or after the window.
const LIMIT: u64 = 128 * 1024 * 1024;
const WINDOW_MS: u64 = 4_000;
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
                    ack_delay: true,
                };
                net::prof::reset();
                let start_tsc = net::prof::tsc();
                let moved = bulk::download(&plan);
                let total_tsc = net::prof::tsc().wrapping_sub(start_tsc);
                let phases = net::prof::ticks();
                net::set_tx_delay_ms(0);
                threads::sleep_ms(SETTLE_MS);
                let kib = rx_buffer / 1024;
                // `net_burst=` on the command line: smoltcp's window cap.
                let tx = if net::sync_tx() { "sync" } else { "async" };
                let burst = match net::burst() {
                    0 => "none".into(),
                    n => alloc::format!("{n}"),
                };
                match moved {
                    Ok(m) => {
                        profile(&phases, total_tsc, m.elapsed_ns + m.first_byte_ns);
                        let ms = (m.elapsed_ns / 1_000_000).max(1);
                        // MB/s to a tenth: bytes per ms is kB/s.
                        let tenths = m.bytes * 10 / ms / 1000;
                        log!(
                            "net-throughput: rtt {rtt} ms, rx {kib} KiB, gap {gap_ms} ms, burst {burst}, tx {tx}: {} bytes in {ms} ms = {}.{} MB/s, first byte {} ms, {} polls",
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
                            "net-throughput: rtt {rtt} ms, rx {kib} KiB, gap {gap_ms} ms, burst {burst}, tx {tx}: failed: {why}"
                        );
                    }
                }
            }
        }
    }
    if failed != 0 {
        return Err("net-throughput: a download failed");
    }
    log!("net-throughput: 8 downloads measured");
    Ok(())
}

/// One download's time by phase: each phase's share of the whole and its cost
/// per received frame in ns; smoltcp's own share is `poll` less what it called.
fn profile(phases: &[u64; net::prof::PHASES], total: u64, wall_ns: u64) {
    use core::sync::atomic::Ordering::Relaxed;
    use net::prof::{NAMES, RX_FRAMES, TX_FRAMES};
    let rx = RX_FRAMES.load(Relaxed).max(1);
    let tx = TX_FRAMES.load(Relaxed);
    let total = total.max(1);
    // ns per tick from the download's wall time; a share needs no rate.
    let ns = |ticks: u64| (u128::from(ticks) * u128::from(wall_ns) / u128::from(total)) as u64;
    let inner = phases[2] + phases[3] + phases[4] + phases[5];
    let smoltcp = phases[1].saturating_sub(inner);
    let mut line =
        alloc::format!("net-throughput: profile: {rx} frames in, {tx} out; per frame in:");
    let shares = NAMES
        .iter()
        .zip(phases.iter())
        .chain(core::iter::once((&"smoltcp itself", &smoltcp)));
    for (name, &ticks) in shares {
        line.push_str(&alloc::format!(
            " {name} {}% {} ns,",
            ticks * 100 / total,
            ns(ticks) / rx
        ));
    }
    log!("{line} total {} ns", ns(total) / rx);
}
