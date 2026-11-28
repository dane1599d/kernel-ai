//! `test=net-throughput` (P5-S2's bar, on main's driver): one download from a
//! plain HTTP server on the host, `blob=ADDR:PORT` (the throughput workflow
//! serves a blob over a TAP device, as the spike did: slirp's TCP to the guest
//! does not scale its window), over a bulk stream ([`stream::BULK_RX_BUFFER`])
//! through the same NET_POLL and NET_RECV paths a ring-3 client takes. Any
//! added round trip is the host's (netem on the TAP device), so the kernel
//! changes nothing for the case. It logs the MB/s from the first byte; the
//! workflow names the round trip and holds the floor.
//!
//! A second, untimed download then checks the card's path (#1623's review):
//! each NET_RECV that returns data polls the interface once, and with polls
//! held off long enough for the card to use every receive buffer, it is
//! kicked again and the download goes on ([`ring_check`]).

use crate::net::stream::{self, ready};
use crate::{hpet, log, net, threads};

/// The kernel's own streams.
const OWNER: u64 = 0;
const REQUEST: &[u8] = b"GET /blob HTTP/1.0\r\nHost: 10.0.2.2\r\n\r\n";
/// A run stops at this many bytes, or this long after its first byte.
const LIMIT: u64 = 128 * 1024 * 1024;
const WINDOW_NS: u64 = 4_000_000_000;
/// No first byte by then is a failure.
const FIRST_BYTE_MS: u64 = 20_000;
const SETUP_WAIT_MS: u64 = 15_000;
/// One NET_POLL's wait between reads.
const POLL_MS: u64 = 100;
/// [`ring_check`]'s download, the point it holds polls off at, and for how
/// long: at 8 MB/s, +80 ms's rate, 1.6 MB comes meanwhile, past the 256
/// buffers of 1.5 KB the card holds.
const RING_BYTES: u64 = 16 * 1024 * 1024;
const RING_HOLD_AT: u64 = 4 * 1024 * 1024;
const RING_HOLD_MS: u64 = 200;
/// Reads that returned data whose polls [`ring_check`] counts.
const POLL_SAMPLES: usize = 16;

pub fn check(cmdline: &str) -> Result<(), &'static str> {
    let (server, port) = cmdline
        .split_whitespace()
        .find_map(|w| net::parse_endpoint(w.strip_prefix("blob=")?))
        .ok_or("net-throughput: no blob= on the command line")?;
    let mut up = false;
    for _ in 0..SETUP_WAIT_MS / 10 {
        if net::status().is_some_and(|s| s.address.is_some()) {
            up = true;
            break;
        }
        threads::sleep_ms(10);
    }
    if !up {
        return Err("net-throughput: the network was not set up in time");
    }
    let id = stream::connect(OWNER, server, port, true, None, None)
        .map_err(|_| "net-throughput: cannot connect to the blob server")?;
    let card = net::card_counts();
    let result = download(id);
    let card_after = net::card_counts();
    let _ = stream::close(OWNER, id);
    let moved = result?;
    log!(
        "net-throughput: card frames={} rx_kicks={} tx_kicks={}",
        card_after.0 - card.0,
        card_after.1 - card.1,
        card_after.2 - card.2
    );
    let ms = (moved.elapsed_ns / 1_000_000).max(1);
    // MB/s to a tenth: bytes per ms is kB/s.
    let tenths = moved.bytes * 10 / ms / 1000;
    log!(
        "net-throughput: {} bytes in {ms} ms = {}.{} MB/s, first byte {} ms, bulk buffer {} KiB",
        moved.bytes,
        tenths / 10,
        tenths % 10,
        moved.first_byte_ns / 1_000_000,
        stream::BULK_RX_BUFFER / 1024
    );
    ring_check(server, port)
}

/// The untimed download: after [`RING_HOLD_AT`] bytes, the polls of
/// [`POLL_SAMPLES`] reads that return data, the least of which must be one
/// (a poll before the read too would make it two); then the network lock
/// held for [`RING_HOLD_MS`], so the card uses every receive buffer and waits
/// for a kick, which must come, and the download must reach [`RING_BYTES`].
fn ring_check(server: core::net::Ipv4Addr, port: u16) -> Result<(), &'static str> {
    let id = stream::connect(OWNER, server, port, true, None, None)
        .map_err(|_| "net-throughput: cannot connect to the blob server again")?;
    let result = ring_download(id);
    let _ = stream::close(OWNER, id);
    let (fewest_polls, kicks_before, kicks_after) = result?;
    log!(
        "net-throughput: the fewest polls of {POLL_SAMPLES} reads with data was {fewest_polls}; rx kicks {kicks_before} before polls were held {RING_HOLD_MS} ms, {kicks_after} after"
    );
    if fewest_polls != 1 {
        return Err("net-throughput: a read with data did not poll the interface exactly once");
    }
    if kicks_after <= kicks_before {
        return Err("net-throughput: the card ran dry and was never kicked again");
    }
    log!(
        "net-throughput: each read polled once, and after the card ran dry it was kicked and the download went on"
    );
    Ok(())
}

fn ring_download(id: u64) -> Result<(u64, u64, u64), &'static str> {
    send_request(id)?;
    let mut buf = alloc::vec![0u8; stream::MAX_CALL];
    let mut bytes = 0u64;
    let mut fewest = u64::MAX;
    let mut samples = 0;
    let mut kicks_before = None;
    let start = hpet::nanos();
    while bytes < RING_BYTES {
        if hpet::nanos().saturating_sub(start) >= FIRST_BYTE_MS * 1_000_000 {
            return Err("net-throughput: the second download stalled");
        }
        let polls = net::polls();
        match stream::recv(OWNER, id, &mut buf) {
            Ok(0) => return Err("net-throughput: the second download ended early"),
            Ok(n) => {
                bytes += n as u64;
                if bytes >= RING_HOLD_AT && samples < POLL_SAMPLES {
                    fewest = fewest.min(net::polls() - polls);
                    samples += 1;
                } else if samples == POLL_SAMPLES && kicks_before.is_none() {
                    kicks_before = Some(net::card_counts().1);
                    net::hold_polls_ms(RING_HOLD_MS);
                }
            }
            Err(kabi::Error::Busy) => {
                let want = ready::READABLE | ready::CLOSED;
                stream::poll(OWNER, id, want, POLL_MS, || false)
                    .map_err(|_| "net-throughput: the second download failed")?;
            }
            Err(_) => return Err("net-throughput: a read of the second download failed"),
        }
    }
    let kicks_before = kicks_before.ok_or("net-throughput: polls were never held")?;
    Ok((fewest, kicks_before, net::card_counts().1))
}

/// Sends the blob's request on `id` once it is connected.
fn send_request(id: u64) -> Result<(), &'static str> {
    let connected = stream::poll(OWNER, id, ready::CONNECTED, FIRST_BYTE_MS, || false)
        .map_err(|_| "net-throughput: the stream failed")?;
    if connected & ready::CONNECTED == 0 {
        return Err("net-throughput: never connected");
    }
    let mut sent = 0;
    while sent < REQUEST.len() {
        sent += stream::send(OWNER, id, &REQUEST[sent..])
            .map_err(|_| "net-throughput: the request did not go")?;
        if sent < REQUEST.len() {
            threads::sleep_ms(1);
        }
    }
    Ok(())
}

struct Moved {
    bytes: u64,
    first_byte_ns: u64,
    elapsed_ns: u64,
}

/// Sends the request, then reads and throws away what comes, timed from the
/// first byte.
fn download(id: u64) -> Result<Moved, &'static str> {
    let start = hpet::nanos();
    send_request(id)?;
    let mut buf = alloc::vec![0u8; stream::MAX_CALL];
    let mut bytes = 0u64;
    let mut first = None;
    loop {
        let now = hpet::nanos();
        if bytes >= LIMIT || first.is_some_and(|f| now.saturating_sub(f) >= WINDOW_NS) {
            break;
        }
        if first.is_none() && now.saturating_sub(start) >= FIRST_BYTE_MS * 1_000_000 {
            return Err("net-throughput: no byte came");
        }
        match stream::recv(OWNER, id, &mut buf) {
            Ok(0) => break,
            Ok(n) => {
                first.get_or_insert(now);
                bytes += n as u64;
            }
            Err(kabi::Error::Busy) => {
                let want = ready::READABLE | ready::CLOSED;
                stream::poll(OWNER, id, want, POLL_MS, || false)
                    .map_err(|_| "net-throughput: the stream failed")?;
            }
            Err(_) => return Err("net-throughput: a read failed"),
        }
    }
    let first = first.ok_or("net-throughput: no byte came")?;
    Ok(Moved {
        bytes,
        first_byte_ns: first.saturating_sub(start),
        elapsed_ns: hpet::nanos().saturating_sub(first),
    })
}
