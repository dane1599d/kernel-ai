//! Spike S07-4's `netd` (ADR 0007's gate; G37, `spike/netd-ring3` only): the
//! virtio network card and smoltcp in ring 3. Started by the kernel's
//! `netd-spike` case with three `Shm` handles in order: the card's BAR
//! (uncached), a DMA arena whose physical address is `dma=`, and a KRNG v1 ring
//! that `netbench` holds too. It fetches `/blob` from `server=` over TCP and
//! serves the body to `netbench` through the ring: each request names a span of
//! the ring's data area, and each response says how many bytes of the body are
//! there. It waits for the card with NETD_WAIT, which the card's MSI-X vector
//! wakes, and for requests on the ring's head with FUTEX_WAIT.

#![no_std]
#![no_main]

extern crate alloc;

mod card;
mod hal;
mod nic;
mod transport;

use alloc::vec;
use alloc::vec::Vec;
use core::ptr::NonNull;
use core::sync::atomic::AtomicU32;
use kabi::handle::{Handle, HandleInfo, Kind};
use kabi::ring::{self, Request, Response};
use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, Ipv4Address};
use user::{arg, args, println, sys};

user::entry!(main);

pub const QUEUE_SIZE: usize = 256;
pub const BUFFER_LEN: usize = 2048;
pub type Nic = nic::VirtioNet<hal::ArenaHal, transport::BarTransport, QUEUE_SIZE>;

/// What the kernel's bulk streams hold (`net::stream::BULK_RX_BUFFER`).
const RX_BUFFER: usize = 1 << 20;
const TX_BUFFER: usize = 16 * 1024;
/// The ring, as the kernel made it (`netd_spike::RING_ENTRIES` and its data).
const RING_ENTRIES: u32 = 256;
const RING_DATA: usize = 1 << 20;
/// The longest wait for the card: smoltcp's timers, and a lost interrupt.
const WAIT_MS: u64 = 5;
/// Offsets of the ring header's heads (`kabi::ring::Header`).
const REQ_HEAD: usize = 12;
const RSP_HEAD: usize = 20;

fn now() -> Instant {
    Instant::from_micros((sys::time_ns() / 1000) as i64)
}

fn number(args: &str, key: &str) -> u64 {
    let value = arg(args, key).unwrap_or("0");
    match value.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).unwrap_or(0),
        None => value.parse().unwrap_or(0),
    }
}

fn main() -> i32 {
    let args = args();
    let mut list = [HandleInfo::default(); 8];
    let count = sys::startup(&mut list).unwrap_or(0).min(list.len());
    let shms: Vec<Handle> = list[..count]
        .iter()
        .filter(|info| info.kind == Kind::Shm as u32)
        .map(|info| Handle(info.handle))
        .collect();
    let [bar, dma, ring_handle] = shms[..] else {
        println!("netd: started with {} Shm handles, not 3", shms.len());
        return 1;
    };
    let (Ok(bar), Ok(dma), Ok(ring_va)) = (
        sys::shm_map(bar, true),
        sys::shm_map(dma, true),
        sys::shm_map(ring_handle, true),
    ) else {
        println!("netd: cannot map its objects");
        return 1;
    };
    hal::init(dma, number(&args, "dma"), number(&args, "dma_pages") * 4096);
    // The ring first, so `netbench` can attach while the card comes up.
    let ring_len = ring::size(RING_ENTRIES, RING_DATA);
    // SAFETY: the ring object is mapped read-write at `ring_va` for at least
    // `ring_len` bytes, and only this process and `netbench` use it.
    let Ok(krng) = (unsafe {
        ring::Ring::init(
            NonNull::new(ring_va as *mut u8).unwrap(),
            ring_len,
            ring::FRAME,
            RING_ENTRIES,
        )
    }) else {
        println!("netd: cannot make the ring");
        return 1;
    };
    let (_, mut server_end) = krng.split();
    let req_head = (ring_va as usize + REQ_HEAD) as *const AtomicU32;
    let rsp_head = (ring_va as usize + RSP_HEAD) as *const AtomicU32;

    let transport = transport::BarTransport::new(
        bar,
        number(&args, "common"),
        number(&args, "notify"),
        number(&args, "mult") as u32,
        number(&args, "isr"),
        number(&args, "device"),
    );
    let Ok(mut nic) = Nic::new(transport) else {
        println!("netd: virtio-net setup failed");
        return 1;
    };
    let interrupts = nic.transport().routed;
    if interrupts {
        nic.receive_interrupts();
    } else {
        nic.disable_interrupts();
    }
    let mut device = card::CardDevice::new(nic);
    let mac = device.mac();
    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    config.random_seed = sys::time_ns();
    let mut iface = Interface::new(config, &mut device, now());
    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::new(IpAddress::v4(10, 0, 2, 15), 24));
    });
    let _ = iface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(10, 0, 2, 2));
    println!(
        "netd: card up in ring 3, MAC {mac:02x?}, {}",
        if interrupts {
            "receive interrupts"
        } else {
            "polled"
        }
    );

    let Some((server, port)) = arg(&args, "server").and_then(endpoint) else {
        println!("netd: no server=");
        return 1;
    };
    let total = number(&args, "bytes");
    let mut sockets = SocketSet::new(Vec::<SocketStorage<'static>>::new());
    let mut socket = tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0u8; RX_BUFFER]),
        tcp::SocketBuffer::new(vec![0u8; TX_BUFFER]),
    );
    socket.set_nagle_enabled(false);
    let local = 49152 + (sys::time_ns() % 8192) as u16;
    if socket
        .connect(iface.context(), (IpAddress::Ipv4(server), port), local)
        .is_err()
    {
        println!("netd: cannot connect");
        return 1;
    }
    let tcp_handle = sockets.add(socket);

    let request = alloc::format!("GET /blob HTTP/1.0\r\nHost: {server}\r\n\r\n");
    let mut sent = false;
    let mut head: Vec<u8> = Vec::new();
    let mut carry: Vec<u8> = Vec::new();
    let mut in_body = false;
    let mut served: u64 = 0;
    let mut waits: u64 = 0;
    loop {
        iface.poll(now(), &mut device, &mut sockets);
        let socket = sockets.get_mut::<tcp::Socket>(tcp_handle);
        if !sent && socket.may_send() {
            let _ = socket.send_slice(request.as_bytes());
            sent = true;
        }
        if !in_body {
            let _ = socket.recv(|data| {
                head.extend_from_slice(data);
                (data.len(), ())
            });
            if let Some(end) = head.windows(4).position(|w| w == b"\r\n\r\n") {
                if !head.starts_with(b"HTTP/1.0 200") && !head.starts_with(b"HTTP/1.1 200") {
                    println!("netd: the server did not answer 200");
                    return 1;
                }
                carry.extend_from_slice(&head[end + 4..]);
                in_body = true;
            }
        }
        // Each request the client has made is a span to fill with the body.
        let mut answered = false;
        while in_body && served < total {
            let ready = carry.len() + socket.recv_queue();
            if ready == 0 {
                break;
            }
            let Ok(Some(req)) = server_end.requests.pop() else {
                break;
            };
            let n = fill(&req, ring_va, ring_len, &mut carry, socket, total - served);
            served += n as u64;
            let rsp = Response {
                tag: req.tag,
                status: 0,
                len: n as u32,
            };
            if !matches!(server_end.responses.push(&rsp), Ok(Ok(()))) {
                println!("netd: the ring's responses are full or corrupt");
                return 1;
            }
            answered = true;
        }
        let waiting_for_client = in_body && served < total && socket.recv_queue() + carry.len() > 0;
        if answered && server_end.client_wants_responses() {
            // SAFETY: the ring's response head, in the mapping.
            let _ = sys::futex_wake(rsp_head, 1);
        }
        // The read just made may open the window: send its update now.
        iface.poll(now(), &mut device, &mut sockets);
        device.refill();
        if served >= total {
            break;
        }
        if waiting_for_client {
            // SAFETY: the ring's request head, in the mapping.
            let seen = unsafe { (*req_head).load(core::sync::atomic::Ordering::Acquire) };
            server_end.want_requests(true);
            if server_end.requests.ready().unwrap_or(0) == 0 {
                let _ = sys::futex_wait(req_head, seen, 1);
            }
            server_end.want_requests(false);
        } else {
            sys::netd_wait(WAIT_MS);
            waits += 1;
        }
    }
    println!("netd: served {served} bytes, {waits} waits for the card");
    // The kernel ends it once `netbench` is done.
    loop {
        sys::sleep_ms(1000);
    }
}

/// Copies up to `req.len` bytes of the body, at most `left`, into the span the
/// request names: what is carried over from the header's read first, then the
/// socket's. How many.
fn fill(
    req: &Request,
    ring_va: u64,
    ring_len: usize,
    carry: &mut Vec<u8>,
    socket: &mut tcp::Socket,
    left: u64,
) -> usize {
    let (start, len) = (req.offset as usize, req.len as usize);
    if start < ring::size(RING_ENTRIES, 0) || start + len > ring_len {
        return 0;
    }
    let want = len.min(left as usize);
    // SAFETY: the span lies in the ring's data area, checked above.
    let out =
        unsafe { core::slice::from_raw_parts_mut((ring_va as usize + start) as *mut u8, want) };
    let mut n = carry.len().min(want);
    out[..n].copy_from_slice(&carry[..n]);
    carry.drain(..n);
    while n < want {
        match socket.recv_slice(&mut out[n..]) {
            Ok(0) | Err(_) => break,
            Ok(got) => n += got,
        }
    }
    n
}

fn endpoint(text: &str) -> Option<(Ipv4Address, u16)> {
    let (ip, port) = text.split_once(':')?;
    let mut octets = [0u8; 4];
    let mut parts = ip.split('.');
    for octet in &mut octets {
        *octet = parts.next()?.parse().ok()?;
    }
    Some((Ipv4Address::from(octets), port.parse().ok()?))
}
