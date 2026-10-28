//! Networking: a polled virtio-net card under smoltcp, set up by DHCP (or a static
//! address, see `setup`), with a DNS resolver (`resolve`), ICMP echo (`ping`), and
//! a one-request-per-connection HTTP exchange used by the LLM client.
//!
//! Every user of the interface polls it through [`Net::poll`], which also hands
//! the DHCP client's news to `setup`, so a lease is never missed whoever polled.
//! Between those, the `net` thread polls it on its own.

mod device;
pub mod ping;
pub mod resolve;
pub mod setup;

use crate::virtio::hal::KernelHal;
use crate::virtio::split::SplitTransport;
use crate::{hpet, log, threads};
use alloc::vec;
use alloc::vec::Vec;
use core::net::Ipv4Addr;
use core::sync::atomic::{AtomicU16, Ordering};
use device::NicDevice;
use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
use smoltcp::socket::tcp;
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress};
use spin::Mutex;
use virtio_drivers::device::net::VirtIONet;

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

type Nic = VirtIONet<KernelHal, SplitTransport, QUEUE_SIZE>;

struct Net {
    device: NicDevice,
    iface: Interface,
    sockets: SocketSet<'static>,
    setup: setup::Setup,
}

// SAFETY: the network state is only ever used under the NET mutex, by one thread at
// a time; nothing in it is tied to a particular CPU.
unsafe impl Send for Net {}

impl Net {
    /// Moves frames both ways and applies whatever the DHCP client learned.
    fn poll(&mut self) {
        self.iface.poll(now(), &mut self.device, &mut self.sockets);
        self.setup
            .after_poll(&mut self.iface, &mut self.sockets, hpet::nanos());
    }
}

static NET: Mutex<Option<Net>> = Mutex::new(None);
static NEXT_PORT: AtomicU16 = AtomicU16::new(FIRST_EPHEMERAL_PORT);

fn now() -> Instant {
    Instant::from_millis((hpet::nanos() / 1_000_000) as i64)
}

/// Brings up the network card the PCI probe found, polled, and starts setting up
/// its address the way the command line said (see [`setup::configure`]).
pub fn init(transport: SplitTransport) -> Result<(), &'static str> {
    let mut nic = Nic::new(transport, BUFFER_LEN).map_err(|_| "net: virtio-net setup failed")?;
    nic.disable_interrupts();
    let mut device = NicDevice::new(nic);
    let mac = device.mac();
    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    // Initial TCP sequence numbers must differ between boots.
    config.random_seed = hpet::nanos()
        ^ u64::from_le_bytes([mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], 0x4B, 0x41]);
    let mut iface = Interface::new(config, &mut device, now());
    // Ports start at a boot-dependent place, so a new boot does not reuse a flow
    // the host may still remember from the previous one.
    NEXT_PORT.store(
        FIRST_EPHEMERAL_PORT + (hpet::nanos() % 8192) as u16,
        Ordering::Relaxed,
    );
    let mut sockets = SocketSet::new(Vec::<SocketStorage<'static>>::new());
    let setup = setup::Setup::start(&mut iface, &mut sockets, hpet::nanos())?;
    log!(
        "net: card up, MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}; {}",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5],
        setup.describe_start()
    );
    *NET.lock() = Some(Net {
        device,
        iface,
        sockets,
        setup,
    });
    threads::spawn("net", setup::run, 0)?;
    Ok(())
}

pub fn is_up() -> bool {
    NET.lock().is_some()
}

fn next_port() -> u16 {
    let port = NEXT_PORT.fetch_add(1, Ordering::Relaxed);
    FIRST_EPHEMERAL_PORT + port.wrapping_sub(FIRST_EPHEMERAL_PORT) % 16384
}

/// Polls until the interface has an address, the deadline passes, or `cancel()`
/// says the caller is gone. Must be called from a thread.
fn wait_for_address(deadline_ns: u64, cancel: &impl Fn() -> bool) -> Result<(), &'static str> {
    loop {
        {
            let mut guard = NET.lock();
            let net = guard.as_mut().ok_or("net: no network card")?;
            net.poll();
            if net.setup.status().address.is_some() {
                return Ok(());
            }
        }
        if cancel() {
            return Err("net: cancelled");
        }
        if hpet::nanos() > deadline_ns {
            return Err("net: no address yet");
        }
        threads::sleep_ms(setup::UNCONFIGURED_POLL_MS);
    }
}

/// Sends `request` to `server`, then reads until the server closes the connection,
/// `max_response` bytes have arrived, `timeout_ms` passes, or `cancel()` says the
/// caller is gone. Parsing what came back is the caller's business. Must be called
/// from a thread (it sleeps between polls). Waits for an address first, within the
/// same time.
pub fn exchange(
    server: (Ipv4Addr, u16),
    request: &[u8],
    timeout_ms: u64,
    max_response: usize,
    cancel: impl Fn() -> bool,
) -> Result<Vec<u8>, &'static str> {
    let deadline = hpet::nanos().saturating_add(timeout_ms.saturating_mul(1_000_000));
    wait_for_address(deadline, &cancel).map_err(|error| match error {
        "net: no address yet" => "net: request timed out",
        other => other,
    })?;
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
    let mut sent = 0;
    let mut response = Vec::new();
    let mut connected = false;
    let mut chunk = [0u8; 2048];
    let result = loop {
        let mut guard = NET.lock();
        let net = guard.as_mut().ok_or("net: no network card")?;
        net.poll();
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
        net.poll();
        net.sockets.remove(handle);
    }
    result.map(|()| response)
}

/// Spike P5-S2 (not for merge): one download's bytes, and the time from its
/// first byte to its last.
pub struct Download {
    pub bytes: u64,
    pub ns: u64,
}

/// How a P5-S2 download runs: the socket's receive buffer (window scaling
/// follows its size), how long the loop sleeps between polls (0: it yields and
/// polls again at once), and when it stops.
pub struct DownloadPlan {
    pub rx_buffer: usize,
    pub poll_gap_ms: u64,
    pub max_bytes: u64,
    pub run_ms: u64,
}

/// Longest wait for a P5-S2 download's connection and first byte.
const DOWNLOAD_CONNECT_MS: u64 = 15_000;

/// Spike P5-S2 (not for merge): sends `request` to `server` and counts what
/// comes back without keeping it, as the polled driver allows: poll, drain the
/// socket, poll again so the window update leaves at once, then sleep. Stops at
/// the server's close, `max_bytes`, or `run_ms` after the first byte; aborts
/// the connection after.
pub fn download(
    server: (Ipv4Addr, u16),
    request: &[u8],
    plan: &DownloadPlan,
) -> Result<Download, &'static str> {
    let connect_by = hpet::nanos() + DOWNLOAD_CONNECT_MS * 1_000_000;
    wait_for_address(connect_by, &|| false)?;
    let handle = {
        let mut guard = NET.lock();
        let net = guard.as_mut().ok_or("net: no network card")?;
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0u8; plan.rx_buffer]),
            tcp::SocketBuffer::new(vec![0u8; TX_BUFFER]),
        );
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
    let (mut sent, mut bytes) = (0, 0u64);
    let (mut first, mut last) = (None::<u64>, 0);
    let result = loop {
        let mut guard = NET.lock();
        let net = guard.as_mut().ok_or("net: no network card")?;
        net.poll();
        let socket = net.sockets.get_mut::<tcp::Socket>(handle);
        if socket.can_send() && sent < request.len() {
            sent += socket.send_slice(&request[sent..]).unwrap_or(0);
        }
        let mut drained = false;
        while let Ok(n) = socket.recv(|buf| (buf.len(), buf.len())) {
            if n == 0 {
                break;
            }
            let now = hpet::nanos();
            first.get_or_insert(now);
            last = now;
            bytes += n as u64;
            drained = true;
        }
        let open = socket.may_recv();
        let state = socket.state();
        if drained {
            net.poll();
        }
        let now = hpet::nanos();
        if bytes >= plan.max_bytes
            || first.is_some_and(|f| now - f >= plan.run_ms * 1_000_000)
            || (bytes > 0 && !open)
        {
            break Ok(());
        }
        if matches!(state, tcp::State::Closed | tcp::State::TimeWait) {
            break Err("net: connection refused or timed out");
        }
        if first.is_none() && now > connect_by {
            break Err("net: no data in time");
        }
        drop(guard);
        match plan.poll_gap_ms {
            0 => threads::yield_now(),
            gap => threads::sleep_ms(gap),
        }
    };
    if let Some(net) = NET.lock().as_mut() {
        net.sockets.get_mut::<tcp::Socket>(handle).abort();
        net.poll();
        net.sockets.remove(handle);
    }
    result.map(|()| Download {
        bytes,
        ns: first.map_or(0, |f| last - f),
    })
}

/// The error a system call reports for an [`exchange`] or a resolve that failed.
pub fn error_code(error: &str) -> kabi::Error {
    match error {
        "net: request timed out" | "dns: timed out" => kabi::Error::TimedOut,
        "net: out of memory" => kabi::Error::NoMemory,
        "dns: no such host" | "dns: the host has no IPv4 address" => kabi::Error::NotFound,
        "dns: no DNS server known" | "net: no network card" => kabi::Error::Unavailable,
        _ if error.starts_with("dns: host name") || error == "dns: empty host name" => {
            kabi::Error::Invalid
        }
        _ => kabi::Error::Network,
    }
}

/// `IPv4:PORT` from the kernel command line, such as `llm=` and `build=`.
pub fn parse_endpoint(text: &str) -> Option<(Ipv4Addr, u16)> {
    let (host, port) = text.rsplit_once(':')?;
    Some((host.parse().ok()?, port.parse().ok().filter(|&p| p > 0)?))
}

/// The network's current setup, for `:net` and the tests.
pub fn status() -> Option<setup::Status> {
    NET.lock().as_ref().map(|net| net.setup.status())
}
