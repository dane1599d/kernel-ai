//! The DNS resolver: A records from the server DHCP named (or `dns=`), asked over
//! UDP with `netcore::dns`, kept in a cache for their TTL. Each server is asked in
//! turn, twice, within the caller's time.

use super::{NET, next_port, wait_for_address};
use crate::{hpet, threads};
use alloc::vec;
use alloc::vec::Vec;
use core::net::Ipv4Addr;
use core::sync::atomic::{AtomicU64, Ordering};
use netcore::cache::Cache;
use netcore::dns::{self, Answer, ReplyError};
use smoltcp::socket::udp;
use smoltcp::wire::IpAddress;
use spin::Mutex;

/// The longest one query waits for its reply.
const TRY_MS: u64 = 1_000;
/// Rounds over the servers.
const ROUNDS: u32 = 2;
/// The time a resolve gets when its caller names none.
pub const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const POLL_MS: u64 = 2;
const PACKETS: usize = 4;
const PACKET_BYTES: usize = 1024;

/// Never held while taking `NET`.
static CACHE: Mutex<Cache> = Mutex::new(Cache::new());
/// Queries sent and answers served from the cache, for `:net` and the tests.
static QUERIES: AtomicU64 = AtomicU64::new(0);
static CACHE_HITS: AtomicU64 = AtomicU64::new(0);

pub struct Resolved {
    pub addresses: Vec<Ipv4Addr>,
    /// Seconds the answer stays good: its TTL, or what is left of it when cached.
    pub ttl_s: u64,
    pub cached: bool,
}

/// (queries sent, answers from the cache).
pub fn counts() -> (u64, u64) {
    (
        QUERIES.load(Ordering::Relaxed),
        CACHE_HITS.load(Ordering::Relaxed),
    )
}

/// Empties the cache: the network or its DNS servers changed.
pub fn forget() {
    CACHE.lock().clear();
}

fn now_ms() -> u64 {
    hpet::nanos() / 1_000_000
}

/// The IPv4 addresses of `host`, a name or a dotted address, within `timeout_ms`
/// or until `cancel()`. Must be called from a thread.
pub fn resolve(
    host: &str,
    timeout_ms: u64,
    cancel: impl Fn() -> bool,
) -> Result<Resolved, &'static str> {
    if let Ok(address) = host.parse::<Ipv4Addr>() {
        return Ok(Resolved {
            addresses: vec![address],
            ttl_s: 0,
            cached: false,
        });
    }
    let host = dns::normalize(host).map_err(|e| e.message())?;
    if let Some((addresses, ttl_s)) = CACHE.lock().get(&host, now_ms()) {
        CACHE_HITS.fetch_add(1, Ordering::Relaxed);
        return Ok(Resolved {
            addresses: addresses.to_vec(),
            ttl_s,
            cached: true,
        });
    }
    let deadline = hpet::nanos().saturating_add(timeout_ms.saturating_mul(1_000_000));
    wait_for_address(deadline, &cancel).map_err(|error| match error {
        "net: no address yet" => "dns: timed out",
        other => other,
    })?;
    let servers: Vec<(Ipv4Addr, u16)> = {
        let guard = NET.lock();
        let net = guard.as_ref().ok_or("net: no network card")?;
        net.setup.status().dns_servers().0.collect()
    };
    if servers.is_empty() {
        return Err("dns: no DNS server known");
    }
    let answer = ask(&host, &servers, deadline, &cancel)?;
    CACHE.lock().put(&host, &answer, now_ms());
    Ok(Resolved {
        ttl_s: answer.ttl.into(),
        addresses: answer.addresses,
        cached: false,
    })
}

/// Asks each server in turn, [`ROUNDS`] times over, until one answers.
fn ask(
    host: &str,
    servers: &[(Ipv4Addr, u16)],
    deadline: u64,
    cancel: &impl Fn() -> bool,
) -> Result<Answer, &'static str> {
    let handle = {
        let mut guard = NET.lock();
        let net = guard.as_mut().ok_or("net: no network card")?;
        let buffer = || {
            udp::PacketBuffer::new(
                vec![udp::PacketMetadata::EMPTY; PACKETS],
                vec![0u8; PACKETS * PACKET_BYTES],
            )
        };
        let mut socket = udp::Socket::new(buffer(), buffer());
        socket
            .bind(next_port())
            .map_err(|_| "dns: cannot open a socket")?;
        net.sockets.add(socket)
    };
    let mut result = Err("dns: timed out");
    // The id only has to differ from query to query and be hard to guess offhand.
    let mut id = (hpet::nanos() ^ (hpet::nanos() >> 17)) as u16;
    'rounds: for _ in 0..ROUNDS {
        for &server in servers {
            id = id.wrapping_add(0x9E37);
            match ask_one(handle, host, server, id, deadline, cancel) {
                Ok(answer) => {
                    result = Ok(answer);
                    break 'rounds;
                }
                // The name's own answer: other servers would say the same.
                Err(Some(error @ (ReplyError::NoSuchHost | ReplyError::NoAddress))) => {
                    result = Err(error.message());
                    break 'rounds;
                }
                Err(Some(error)) => result = Err(error.message()),
                Err(None) => {}
            }
            if cancel() {
                result = Err("net: cancelled");
                break 'rounds;
            }
            if hpet::nanos() > deadline {
                break 'rounds;
            }
        }
    }
    if let Some(net) = NET.lock().as_mut() {
        net.sockets.remove(handle);
    }
    result
}

/// One query to one server; Err(None) when no reply came in time.
fn ask_one(
    handle: smoltcp::iface::SocketHandle,
    host: &str,
    server: (Ipv4Addr, u16),
    id: u16,
    deadline: u64,
    cancel: &impl Fn() -> bool,
) -> Result<Answer, Option<ReplyError>> {
    let query = dns::query(id, host);
    let give_up = deadline.min(hpet::nanos() + TRY_MS * 1_000_000);
    let endpoint = (IpAddress::Ipv4(server.0), server.1);
    let mut sent = false;
    let mut reply = [0u8; PACKET_BYTES];
    loop {
        {
            let mut guard = NET.lock();
            let Some(net) = guard.as_mut() else {
                return Err(None);
            };
            let socket = net.sockets.get_mut::<udp::Socket>(handle);
            if !sent && socket.send_slice(&query, endpoint).is_ok() {
                sent = true;
                QUERIES.fetch_add(1, Ordering::Relaxed);
            }
            net.poll();
            let socket = net.sockets.get_mut::<udp::Socket>(handle);
            while let Ok((len, meta)) = socket.recv_slice(&mut reply) {
                if meta.endpoint.addr != endpoint.0 || meta.endpoint.port != server.1 {
                    continue;
                }
                match dns::parse_reply(id, host, &reply[..len]) {
                    Err(ReplyError::NotOurs | ReplyError::Malformed) => {}
                    other => return other.map_err(Some),
                }
            }
        }
        if cancel() || hpet::nanos() > give_up {
            return Err(None);
        }
        threads::sleep_ms(POLL_MS);
    }
}
