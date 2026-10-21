//! ICMP echo: `ping` sends requests one interval apart and times each reply.

use super::{NET, wait_for_address};
use crate::{hpet, threads};
use alloc::vec;
use core::net::Ipv4Addr;
use core::sync::atomic::{AtomicU16, Ordering};
use netcore::ping::Stats;
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::socket::icmp;
use smoltcp::wire::{Icmpv4Packet, Icmpv4Repr, IpAddress};

/// The time between requests, and the longest one waits for its reply.
pub const INTERVAL_MS: u64 = 1_000;
pub const MAX_COUNT: u16 = 20;
const POLL_MS: u64 = 1;
const PAYLOAD: &[u8] = b"Kernel AI ping!!";
const PACKETS: usize = 4;
const PACKET_BYTES: usize = 256;

/// Each ping gets its own identifier, so two at once never take each other's replies.
static NEXT_IDENT: AtomicU16 = AtomicU16::new(0x4B41);

/// What became of one request.
pub enum Outcome {
    /// The reply came after this many microseconds.
    Reply(u64),
    Lost,
}

/// Pings `target` `count` times, calling `each(seq, outcome)` as each request
/// ends; stops early when `cancel()` says so. Must be called from a thread.
pub fn ping(
    target: Ipv4Addr,
    count: u16,
    mut each: impl FnMut(u16, Outcome),
    cancel: impl Fn() -> bool,
) -> Result<Stats, &'static str> {
    let deadline = hpet::nanos() + INTERVAL_MS * 1_000_000;
    wait_for_address(deadline, &cancel)?;
    let ident = NEXT_IDENT.fetch_add(1, Ordering::Relaxed);
    let handle = {
        let mut guard = NET.lock();
        let net = guard.as_mut().ok_or("net: no network card")?;
        let buffer = || {
            icmp::PacketBuffer::new(
                vec![icmp::PacketMetadata::EMPTY; PACKETS],
                vec![0u8; PACKETS * PACKET_BYTES],
            )
        };
        let mut socket = icmp::Socket::new(buffer(), buffer());
        socket
            .bind(icmp::Endpoint::Ident(ident))
            .map_err(|_| "ping: cannot open a socket")?;
        net.sockets.add(socket)
    };
    let mut stats = Stats::default();
    let mut result = Ok(());
    for seq in 0..count.min(MAX_COUNT) {
        if cancel() {
            break;
        }
        let started = hpet::nanos();
        let outcome = match echo(handle, target, ident, seq, started, &cancel) {
            Ok(outcome) => outcome,
            Err(error) => {
                result = Err(error);
                break;
            }
        };
        stats.sent();
        if let Outcome::Reply(us) = outcome {
            stats.received(us);
        }
        each(seq, outcome);
        if seq + 1 < count.min(MAX_COUNT) {
            let next = started + INTERVAL_MS * 1_000_000;
            threads::sleep_until_unless(next, &cancel);
        }
    }
    if let Some(net) = NET.lock().as_mut() {
        net.sockets.remove(handle);
    }
    result.map(|()| stats)
}

/// Sends request `seq` and waits up to [`INTERVAL_MS`] for its reply.
fn echo(
    handle: smoltcp::iface::SocketHandle,
    target: Ipv4Addr,
    ident: u16,
    seq: u16,
    started: u64,
    cancel: &impl Fn() -> bool,
) -> Result<Outcome, &'static str> {
    let request = Icmpv4Repr::EchoRequest {
        ident,
        seq_no: seq,
        data: PAYLOAD,
    };
    let caps = ChecksumCapabilities::default();
    let mut sent = false;
    let give_up = started + INTERVAL_MS * 1_000_000;
    loop {
        {
            let mut guard = NET.lock();
            let net = guard.as_mut().ok_or("net: no network card")?;
            let socket = net.sockets.get_mut::<icmp::Socket>(handle);
            if !sent {
                let buffer = socket
                    .send(request.buffer_len(), IpAddress::Ipv4(target))
                    .map_err(|_| "ping: cannot send")?;
                request.emit(&mut Icmpv4Packet::new_unchecked(buffer), &caps);
                sent = true;
            }
            net.poll();
            let socket = net.sockets.get_mut::<icmp::Socket>(handle);
            while let Ok((bytes, from)) = socket.recv() {
                let Ok(packet) = Icmpv4Packet::new_checked(bytes) else {
                    continue;
                };
                let reply = Icmpv4Repr::parse(&packet, &caps);
                if from == IpAddress::Ipv4(target)
                    && let Ok(Icmpv4Repr::EchoReply {
                        ident: got_ident,
                        seq_no,
                        data,
                    }) = reply
                    && got_ident == ident
                    && seq_no == seq
                    && data == PAYLOAD
                {
                    return Ok(Outcome::Reply((hpet::nanos() - started) / 1000));
                }
            }
        }
        if cancel() || hpet::nanos() > give_up {
            return Ok(Outcome::Lost);
        }
        threads::sleep_ms(POLL_MS);
    }
}
