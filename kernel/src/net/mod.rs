//! Networking: a polled virtio-net card under smoltcp, on QEMU's user-mode network
//! (static 10.0.2.15/24, gateway 10.0.2.2), and a one-request-per-connection HTTP
//! exchange used by the LLM client.

mod device;

use crate::virtio::hal::KernelHal;
use crate::{hpet, log, threads};
use alloc::vec;
use alloc::vec::Vec;
use core::net::Ipv4Addr;
use core::sync::atomic::{AtomicU16, Ordering};
use device::NicDevice;
use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
use smoltcp::socket::tcp;
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr};
use spin::Mutex;
use virtio_drivers::device::net::VirtIONet;
use virtio_drivers::transport::pci::PciTransport;

const GUEST_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 15);
const GATEWAY: Ipv4Addr = Ipv4Addr::new(10, 0, 2, 2);
const PREFIX_LEN: u8 = 24;
const RX_BUFFER: usize = 64 * 1024;
const TX_BUFFER: usize = 16 * 1024;
/// Longest wait between polls: without interrupts this bounds how late a frame is seen.
const MAX_POLL_GAP_MS: u64 = 2;
const FIRST_EPHEMERAL_PORT: u16 = 49152;
/// Receive and transmit queue depth: a power of two, kept small because the driver
/// builds its queue arrays on the stack.
const QUEUE_SIZE: usize = 16;
/// Per-packet buffer: at least 1526 bytes and a multiple of 8.
const BUFFER_LEN: usize = 2048;

type Nic = VirtIONet<KernelHal, PciTransport, QUEUE_SIZE>;

struct Net {
    device: NicDevice,
    iface: Interface,
    sockets: SocketSet<'static>,
}

// SAFETY: the network state is only ever used under the NET mutex, by one thread at
// a time; nothing in it is tied to a particular CPU.
unsafe impl Send for Net {}

static NET: Mutex<Option<Net>> = Mutex::new(None);
static NEXT_PORT: AtomicU16 = AtomicU16::new(FIRST_EPHEMERAL_PORT);

fn now() -> Instant {
    Instant::from_millis((hpet::nanos() / 1_000_000) as i64)
}

/// Brings up the network card the PCI probe found, polled, and configures the
/// interface.
pub fn init(transport: PciTransport) -> Result<(), &'static str> {
    let mut nic = Nic::new(transport, BUFFER_LEN).map_err(|_| "net: virtio-net setup failed")?;
    nic.disable_interrupts();
    let mut device = NicDevice::new(nic);
    let mac = device.mac();
    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    // Initial TCP sequence numbers must differ between boots.
    config.random_seed = hpet::nanos()
        ^ u64::from_le_bytes([mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], 0x4B, 0x41]);
    let mut iface = Interface::new(config, &mut device, now());
    iface.update_ip_addrs(|addresses| {
        let _ = addresses.push(IpCidr::new(IpAddress::Ipv4(GUEST_IP), PREFIX_LEN));
    });
    iface
        .routes_mut()
        .add_default_ipv4_route(GATEWAY)
        .map_err(|_| "net: route table full")?;
    // Ports start at a boot-dependent place, so a new boot does not reuse a flow
    // the host may still remember from the previous one.
    NEXT_PORT.store(
        FIRST_EPHEMERAL_PORT + (hpet::nanos() % 8192) as u16,
        Ordering::Relaxed,
    );
    let sockets = SocketSet::new(Vec::<SocketStorage<'static>>::new());
    *NET.lock() = Some(Net {
        device,
        iface,
        sockets,
    });
    log!(
        "net: up as {GUEST_IP}/{PREFIX_LEN} via {GATEWAY}, MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5]
    );
    Ok(())
}

pub fn is_up() -> bool {
    NET.lock().is_some()
}

fn next_port() -> u16 {
    let port = NEXT_PORT.fetch_add(1, Ordering::Relaxed);
    FIRST_EPHEMERAL_PORT + port.wrapping_sub(FIRST_EPHEMERAL_PORT) % 16384
}

/// Sends `request` to `server`, then reads until the server closes the connection,
/// `max_response` bytes have arrived, `timeout_ms` passes, or `cancel()` says the
/// caller is gone. Parsing what came back is the caller's business. Must be called
/// from a thread (it sleeps between polls).
pub fn exchange(
    server: (Ipv4Addr, u16),
    request: &[u8],
    timeout_ms: u64,
    max_response: usize,
    cancel: impl Fn() -> bool,
) -> Result<Vec<u8>, &'static str> {
    let handle = {
        let mut guard = NET.lock();
        let net = guard.as_mut().ok_or("net: no network card")?;
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0u8; RX_BUFFER]),
            tcp::SocketBuffer::new(vec![0u8; TX_BUFFER]),
        );
        socket.set_timeout(Some(Duration::from_millis(timeout_ms)));
        socket.set_nagle_enabled(false);
        socket
            .connect(
                net.iface.context(),
                (IpAddress::Ipv4(server.0), server.1),
                next_port(),
            )
            .map_err(|_| "net: cannot start the connection")?;
        net.sockets.add(socket)
    };
    let deadline = hpet::nanos().saturating_add(timeout_ms.saturating_mul(1_000_000));
    let mut sent = 0;
    let mut response = Vec::new();
    let mut connected = false;
    let mut chunk = [0u8; 2048];
    let result = loop {
        let mut guard = NET.lock();
        let net = guard.as_mut().ok_or("net: no network card")?;
        net.iface.poll(now(), &mut net.device, &mut net.sockets);
        let socket = net.sockets.get_mut::<tcp::Socket>(handle);
        if matches!(
            socket.state(),
            tcp::State::Established | tcp::State::CloseWait
        ) {
            connected = true;
        }
        if socket.can_send() && sent < request.len() {
            sent += socket.send_slice(&request[sent..]).unwrap_or(0);
        }
        let mut failed = None;
        while socket.can_recv() && response.len() < max_response {
            let room = (max_response - response.len()).min(chunk.len());
            match socket.recv_slice(&mut chunk[..room]) {
                Ok(n) if response.try_reserve(n).is_ok() => response.extend_from_slice(&chunk[..n]),
                Ok(_) => {
                    failed = Some("net: out of memory");
                    break;
                }
                Err(_) => break,
            }
        }
        if let Some(error) = failed {
            break Err(error);
        }
        // A close ends the exchange even with nothing received: no more data can come.
        let closed = connected && !socket.may_recv();
        if closed || response.len() >= max_response {
            break Ok(());
        }
        if matches!(socket.state(), tcp::State::Closed | tcp::State::TimeWait) {
            break if connected {
                Ok(())
            } else {
                Err("net: connection refused or timed out")
            };
        }
        if cancel() {
            break Err("net: cancelled");
        }
        if hpet::nanos() > deadline {
            break Err("net: request timed out");
        }
        drop(guard);
        threads::sleep_ms(MAX_POLL_GAP_MS);
    };
    // Close politely if the exchange finished, abort otherwise, and flush that out.
    if let Some(net) = NET.lock().as_mut() {
        let socket = net.sockets.get_mut::<tcp::Socket>(handle);
        if result.is_ok() {
            socket.close();
        } else {
            socket.abort();
        }
        net.iface.poll(now(), &mut net.device, &mut net.sockets);
        net.sockets.remove(handle);
    }
    result.map(|()| response)
}

/// `IPv4:PORT` from the kernel command line, such as `llm=` and `build=`; the
/// kernel has no DNS.
pub fn parse_endpoint(text: &str) -> Option<(Ipv4Addr, u16)> {
    let (host, port) = text.rsplit_once(':')?;
    Some((host.parse().ok()?, port.parse().ok().filter(|&p| p > 0)?))
}
