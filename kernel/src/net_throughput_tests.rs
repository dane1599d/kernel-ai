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
//! each NET_RECV that returns data polls the interface once, and once the
//! card has used every receive buffer it held and is given them back, it is
//! kicked and the download goes on ([`ring_check`]).

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
/// [`ring_check`]'s download, and the point it stops giving the card receive
/// buffers back at.
const RING_BYTES: u64 = 16 * 1024 * 1024;
const RING_HOLD_AT: u64 = 4 * 1024 * 1024;
/// How long the card may take to use the buffers it holds, and how long it is
/// left dry after, so the frames still on the way reach it and it asks for a
/// kick (its `avail_event`) before it gets buffers back.
const DRY_WITHIN_MS: u64 = 10_000;
const DRY_FOR_MS: u64 = 100;
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
/// (a poll before the read too would make it two). Then the card is given no
/// receive buffers back until it holds none, however many frames the sender
/// keeps in flight; it must hold none, and once it gets them back it must be
/// kicked and the download must reach [`RING_BYTES`].
fn ring_check(server: core::net::Ipv4Addr, port: u16) -> Result<(), &'static str> {
    let id = stream::connect(OWNER, server, port, true, None, None)
        .map_err(|_| "net-throughput: cannot connect to the blob server again")?;
    let result = ring_download(id);
    // Whatever happened, every later poll refills again.
    net::withhold_receive_buffers(false);
    let _ = stream::close(OWNER, id);
    let ring = result?;
    let (_, event_idx) = net::card_ring().ok_or("net-throughput: the card is gone")?;
    log!(
        "net-throughput: the fewest polls of {POLL_SAMPLES} reads with data was {}; event_idx={event_idx}; the card held {} receive buffers once dry; rx kicks {} before the buffers went back, {} after; {} frames came by the first read with data after",
        ring.fewest_polls,
        ring.held_dry,
        ring.kicks_before,
        ring.kicks_after,
        ring.first_frames
    );
    if ring.fewest_polls != 1 {
        return Err("net-throughput: a read with data did not poll the interface exactly once");
    }
    if ring.held_dry != 0 {
        return Err("net-throughput: the card never used every receive buffer it held");
    }
    if ring.kicks_after <= ring.kicks_before {
        return Err("net-throughput: the card ran dry and was never kicked again");
    }
    log!(
        "net-throughput: each read polled once, and after the card ran dry it was kicked and the download went on"
    );
    Ok(())
}

/// What [`ring_download`] saw.
struct Ring {
    fewest_polls: u64,
    /// Receive buffers the card held when it was given them back: 0 once dry.
    held_dry: usize,
    kicks_before: u64,
    kicks_after: u64,
    /// Frames the card received from the buffers going back to the first read
    /// with data after.
    first_frames: u64,
}

fn ring_download(id: u64) -> Result<Ring, &'static str> {
    send_request(id)?;
    let mut buf = alloc::vec![0u8; stream::MAX_CALL];
    let mut bytes = 0u64;
    let mut fewest = u64::MAX;
    let mut samples = 0;
    // Set once the buffers went back: held then, kicks before, frames before.
    let mut given_back: Option<(usize, u64, u64)> = None;
    let mut first_frames = None;
    let mut dry_since: Option<u64> = None;
    let mut withheld_at = None;
    let start = hpet::nanos();
    while bytes < RING_BYTES {
        let now = hpet::nanos();
        if now.saturating_sub(start) >= FIRST_BYTE_MS * 1_000_000 {
            return Err("net-throughput: the second download stalled");
        }
        if let Some(at) = withheld_at
            && given_back.is_none()
        {
            let (held, _) = net::card_ring().ok_or("net-throughput: the card is gone")?;
            if held == 0 {
                dry_since.get_or_insert(now);
            }
            let dry_long =
                dry_since.is_some_and(|d| now.saturating_sub(d) >= DRY_FOR_MS * 1_000_000);
            let too_long = now.saturating_sub(at) >= DRY_WITHIN_MS * 1_000_000;
            if dry_long || too_long {
                let (frames, kicks, _) = net::card_counts();
                given_back = Some((held, kicks, frames));
                net::withhold_receive_buffers(false);
                if !dry_long {
                    break;
                }
            }
        }
        let polls = net::polls();
        match stream::recv(OWNER, id, &mut buf) {
            Ok(0) => return Err("net-throughput: the second download ended early"),
            Ok(n) => {
                bytes += n as u64;
                if let Some((_, _, frames)) = given_back
                    && first_frames.is_none()
                {
                    first_frames = Some(net::card_counts().0 - frames);
                }
                if bytes >= RING_HOLD_AT && samples < POLL_SAMPLES {
                    fewest = fewest.min(net::polls() - polls);
                    samples += 1;
                    if samples == POLL_SAMPLES {
                        net::withhold_receive_buffers(true);
                        withheld_at = Some(hpet::nanos());
                    }
                }
            }
            Err(kabi::Error::Busy) => {
                let want = ready::READABLE | ready::CLOSED;
                // Dry, the card takes nothing in, so this waits out its gap.
                stream::poll(OWNER, id, want, POLL_MS.min(DRY_FOR_MS / 4), || false)
                    .map_err(|_| "net-throughput: the second download failed")?;
            }
            Err(_) => return Err("net-throughput: a read of the second download failed"),
        }
    }
    let (held_dry, kicks_before, _) =
        given_back.ok_or("net-throughput: the buffers were never withheld")?;
    Ok(Ring {
        fewest_polls: fewest,
        held_dry,
        kicks_before,
        kicks_after: net::card_counts().1,
        first_frames: first_frames.unwrap_or(0),
    })
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
