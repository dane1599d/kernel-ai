//! Spike P5-S2 (`net-throughput`): one download from a host, read and thrown
//! away, timed from its first byte, over a receive buffer of a given size and
//! the polled driver with a given gap between polls.

use super::{NET, next_port, wait_for_address};
use crate::{hpet, threads};
use alloc::vec;
use core::net::Ipv4Addr;
use smoltcp::socket::tcp;
use smoltcp::time::Duration;
use smoltcp::wire::IpAddress;

const TX_BUFFER: usize = 16 * 1024;

/// How a download should run.
pub struct Plan<'a> {
    pub server: (Ipv4Addr, u16),
    pub request: &'a [u8],
    /// The socket's receive buffer, in bytes.
    pub rx_buffer: usize,
    /// Sleep between polls in ms; 0 yields the CPU instead.
    pub gap_ms: u64,
    /// Stop after this many bytes.
    pub limit: u64,
    /// Stop this long after the first byte.
    pub window_ms: u64,
    /// Give up if no byte came by then.
    pub timeout_ms: u64,
    /// smoltcp's delayed ACK: its default 10 ms, or off.
    pub ack_delay: bool,
}

/// What one download moved.
pub struct Moved {
    pub bytes: u64,
    /// From the connect call to the first byte.
    pub first_byte_ns: u64,
    /// From the first byte to the last.
    pub elapsed_ns: u64,
    pub polls: u64,
}

pub fn download(plan: &Plan) -> Result<Moved, &'static str> {
    let start = hpet::nanos();
    let deadline = start.saturating_add(plan.timeout_ms.saturating_mul(1_000_000));
    wait_for_address(deadline, &|| false)?;
    let handle = {
        let mut guard = NET.lock();
        let net = guard.as_mut().ok_or("net: no network card")?;
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0u8; plan.rx_buffer]),
            tcp::SocketBuffer::new(vec![0u8; TX_BUFFER]),
        );
        socket.set_timeout(Some(Duration::from_millis(plan.timeout_ms)));
        socket.set_nagle_enabled(false);
        if !plan.ack_delay {
            socket.set_ack_delay(None);
        }
        socket
            .connect(
                net.iface.context(),
                (IpAddress::Ipv4(plan.server.0), plan.server.1),
                next_port(),
            )
            .map_err(|_| "net: cannot start the connection")?;
        net.sockets.add(socket)
    };
    let connect_ns = hpet::nanos();
    let mut sent = 0;
    let mut moved = Moved {
        bytes: 0,
        first_byte_ns: 0,
        elapsed_ns: 0,
        polls: 0,
    };
    let mut first = 0;
    let mut connected = false;
    let mut chunk = vec![0u8; 8 * 1024];
    let result = loop {
        let mut guard = NET.lock();
        let net = guard.as_mut().ok_or("net: no network card")?;
        net.poll();
        moved.polls += 1;
        let socket = net.sockets.get_mut::<tcp::Socket>(handle);
        if matches!(
            socket.state(),
            tcp::State::Established | tcp::State::CloseWait
        ) {
            connected = true;
        }
        if socket.can_send() && sent < plan.request.len() {
            sent += socket.send_slice(&plan.request[sent..]).unwrap_or(0);
        }
        while socket.can_recv() {
            match socket.recv_slice(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if first == 0 {
                        first = hpet::nanos();
                    }
                    moved.bytes += n as u64;
                }
            }
        }
        let now = hpet::nanos();
        if first != 0 {
            moved.elapsed_ns = now - first;
        }
        let window_over = first != 0 && moved.elapsed_ns >= plan.window_ms * 1_000_000;
        if moved.bytes >= plan.limit || window_over || (connected && !socket.may_recv()) {
            break Ok(());
        }
        if matches!(socket.state(), tcp::State::Closed | tcp::State::TimeWait) {
            break if connected {
                Ok(())
            } else {
                Err("net: connection refused or timed out")
            };
        }
        if first == 0 && now > deadline {
            break Err("net: no byte came in time");
        }
        drop(guard);
        match plan.gap_ms {
            0 => threads::yield_now(),
            gap => threads::sleep_ms(gap),
        }
    };
    if let Some(net) = NET.lock().as_mut() {
        net.sockets.get_mut::<tcp::Socket>(handle).abort();
        net.poll();
        net.sockets.remove(handle);
    }
    moved.first_byte_ns = first.saturating_sub(connect_ns);
    result.map(|()| moved)
}
