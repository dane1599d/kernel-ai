//! TCP streams on handles (P5 task 2, ADR 0001 and 0007): a process connects
//! on its `NetDest`, and gets a `Stream` handle to send, receive, wait on and
//! close. The sockets live in the one interface's set; this table says whose
//! each is.
//!
//! - **Where it may go** is the `NetDest`'s to say ([`super::dest::may_connect`]).
//! - **Limits:** [`MAX_PER_PROCESS`] streams a process and [`MAX_STREAMS`] in
//!   all, [`MAX_CALL`] bytes a send or a receive, and [`MAX_BULK`] bulk streams
//!   at once, [`MAX_BULK_PER_PROCESS`] of them one process's.
//! - **Closing the handle** (HANDLE_CLOSE) closes the stream, as NET_CLOSE does.
//! - **Buffers** (spike P5-S2): a stream receives into [`RX_BUFFER`], a bulk one
//!   into [`BULK_RX_BUFFER`]; smoltcp offers the peer a window scaled to fit.
//! - **A process that ends** has every stream it held aborted ([`on_exit`]). A
//!   closed stream lingers only until its close is done ([`reap`]).
//!
//! Locks: `NET`, then `STREAMS`, never the other way.

use super::{NET, Net, next_port};
use crate::hpet;
use alloc::vec::Vec;
use core::net::Ipv4Addr;
use kabi::Error;
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp;
use smoltcp::wire::IpAddress;
use spin::Mutex;

/// Streams one process may hold open.
pub const MAX_PER_PROCESS: usize = kabi::net::STREAMS_PER_PROCESS;
/// Streams open at once, in all.
pub const MAX_STREAMS: usize = kabi::net::STREAMS_MAX;
/// The most bytes one NET_SEND takes or one NET_RECV gives.
pub const MAX_CALL: usize = kabi::net::STREAM_MAX_CALL;
/// A stream's receive buffer.
pub const RX_BUFFER: usize = 256 * 1024;
/// A bulk stream's: about 50 MB/s at a 20 ms round trip, once the window is
/// scaled (P5-S2).
pub const BULK_RX_BUFFER: usize = 1024 * 1024;
/// Bulk streams at once, in all, and for one process: no one process takes
/// every bulk buffer.
pub const MAX_BULK: usize = 4;
pub const MAX_BULK_PER_PROCESS: usize = 2;
const TX_BUFFER: usize = 64 * 1024;
/// Closed streams whose close is still going out; the oldest is aborted past it.
const MAX_CLOSING: usize = 32;
/// The longest NET_POLL wait, and the gap between its polls.
pub const MAX_WAIT_MS: u64 = 60_000;
const POLL_GAP_MS: u64 = 1;

/// What NET_POLL reports.
pub mod ready {
    pub const CONNECTED: u64 = kabi::net::STREAM_CONNECTED;
    pub const READABLE: u64 = kabi::net::STREAM_READABLE;
    pub const WRITABLE: u64 = kabi::net::STREAM_WRITABLE;
    pub const CLOSED: u64 = kabi::net::STREAM_CLOSED;
    pub const FAILED: u64 = kabi::net::STREAM_FAILED;
}

struct Stream {
    owner: u64,
    socket: SocketHandle,
    bulk: bool,
    /// It has been established at least once.
    connected: bool,
}

struct Streams {
    open: Vec<Option<Stream>>,
    closing: Vec<SocketHandle>,
}

static STREAMS: Mutex<Streams> = Mutex::new(Streams {
    open: Vec::new(),
    closing: Vec::new(),
});

/// A stream's object id: its index, plus 1.
fn index(id: u64) -> Option<usize> {
    usize::try_from(id.checked_sub(1)?).ok()
}

/// Opens a stream for `owner` to `addr`:`port`, which the caller checked
/// against its `NetDest`; `bulk` gives it the larger buffer. Its id.
pub fn connect(owner: u64, addr: Ipv4Addr, port: u16, bulk: bool) -> Result<u64, Error> {
    let mut guard = NET.lock();
    let net = guard.as_mut().ok_or(Error::Unavailable)?;
    if net.setup.status().address.is_none() {
        return Err(Error::Unavailable);
    }
    let mut streams = STREAMS.lock();
    let live = streams.open.iter().flatten();
    let (mine, bulks, my_bulks) = live.fold((0, 0, 0), |(m, b, mb), s| {
        let own = s.owner == owner;
        (
            m + usize::from(own),
            b + usize::from(s.bulk),
            mb + usize::from(own && s.bulk),
        )
    });
    let total = streams.open.iter().flatten().count();
    let bulk_full = bulks >= MAX_BULK || my_bulks >= MAX_BULK_PER_PROCESS;
    if mine >= MAX_PER_PROCESS || total >= MAX_STREAMS || (bulk && bulk_full) {
        return Err(Error::Busy);
    }
    let rx_len = if bulk { BULK_RX_BUFFER } else { RX_BUFFER };
    let socket = tcp::Socket::new(
        tcp::SocketBuffer::new(buffer(rx_len)?),
        tcp::SocketBuffer::new(buffer(TX_BUFFER)?),
    );
    let socket = start(net, socket, addr, port)?;
    let stream = Stream {
        owner,
        socket,
        bulk,
        connected: false,
    };
    let at = match streams.open.iter().position(Option::is_none) {
        Some(at) => at,
        None => {
            streams.open.push(None);
            streams.open.len() - 1
        }
    };
    streams.open[at] = Some(stream);
    drop(streams);
    net.poll();
    Ok(at as u64 + 1)
}

fn buffer(len: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| Error::NoMemory)?;
    bytes.resize(len, 0);
    Ok(bytes)
}

fn start(
    net: &mut Net,
    mut socket: tcp::Socket<'static>,
    addr: Ipv4Addr,
    port: u16,
) -> Result<SocketHandle, Error> {
    socket.set_nagle_enabled(false);
    socket
        .connect(
            net.iface.context(),
            (IpAddress::Ipv4(addr), port),
            next_port(),
        )
        .map_err(|_| Error::Invalid)?;
    Ok(net.sockets.add(socket))
}

/// Runs `f` on `owner`'s stream `id` with the interface just polled.
fn with<R>(
    owner: u64,
    id: u64,
    f: impl FnOnce(&mut tcp::Socket, &mut bool) -> R,
) -> Result<R, Error> {
    let mut guard = NET.lock();
    let net = guard.as_mut().ok_or(Error::Unavailable)?;
    net.poll();
    let mut streams = STREAMS.lock();
    let stream = index(id)
        .and_then(|at| streams.open.get_mut(at)?.as_mut())
        .filter(|s| s.owner == owner)
        .ok_or(Error::BadHandle)?;
    let socket = net.sockets.get_mut::<tcp::Socket>(stream.socket);
    if matches!(
        socket.state(),
        tcp::State::Established | tcp::State::CloseWait
    ) {
        stream.connected = true;
    }
    let result = f(socket, &mut stream.connected);
    drop(streams);
    net.poll();
    Ok(result)
}

/// What the stream can do now ([`ready`]).
fn state(socket: &tcp::Socket, connected: bool) -> u64 {
    let mut bits = 0;
    if connected {
        bits |= ready::CONNECTED;
    }
    if socket.can_recv() {
        bits |= ready::READABLE;
    }
    if connected && socket.can_send() {
        bits |= ready::WRITABLE;
    }
    if connected && !socket.may_recv() && !socket.can_recv() {
        bits |= ready::CLOSED;
    }
    let gone = matches!(socket.state(), tcp::State::Closed | tcp::State::TimeWait);
    if gone && !connected {
        bits |= ready::FAILED;
    }
    bits
}

/// NET_SEND: queues up to [`MAX_CALL`] bytes of `data`; how many it took,
/// which is 0 while the send buffer is full. Dead once the stream can no
/// longer send.
pub fn send(owner: u64, id: u64, data: &[u8]) -> Result<usize, Error> {
    if data.len() > MAX_CALL {
        return Err(Error::TooBig);
    }
    with(owner, id, |socket, connected| {
        if !*connected {
            return Err(Error::Busy);
        }
        if !socket.may_send() {
            return Err(Error::Dead);
        }
        socket.send_slice(data).map_err(|_| Error::Dead)
    })?
}

/// NET_RECV: up to `out.len()` (at most [`MAX_CALL`]) bytes received; 0 once
/// the peer closed and everything was read; Busy while none has come yet.
pub fn recv(owner: u64, id: u64, out: &mut [u8]) -> Result<usize, Error> {
    let cap = out.len().min(MAX_CALL);
    with(owner, id, |socket, connected| {
        match socket.recv_slice(&mut out[..cap]) {
            Ok(0) | Err(tcp::RecvError::InvalidState) if *connected && !socket.may_recv() => Ok(0),
            Ok(0) => Err(Error::Busy),
            Ok(n) => Ok(n),
            Err(tcp::RecvError::Finished) => Ok(0),
            Err(tcp::RecvError::InvalidState) => Err(Error::Busy),
        }
    })?
}

/// NET_POLL: waits up to `timeout_ms` (at most [`MAX_WAIT_MS`]) until the
/// stream is any of `want` ([`ready`]) or has failed, or `cancel()`; what it
/// is then, which may be none of `want` after the timeout.
pub fn poll(
    owner: u64,
    id: u64,
    want: u64,
    timeout_ms: u64,
    cancel: impl Fn() -> bool,
) -> Result<u64, Error> {
    let deadline = hpet::nanos().saturating_add(timeout_ms.min(MAX_WAIT_MS) * 1_000_000);
    loop {
        let bits = with(owner, id, |socket, connected| state(socket, *connected))?;
        if bits & (want | ready::FAILED) != 0 || hpet::nanos() >= deadline {
            return Ok(bits);
        }
        if cancel() {
            return Err(Error::Interrupted);
        }
        crate::threads::sleep_ms(POLL_GAP_MS);
    }
}

/// NET_CLOSE: closes `owner`'s stream `id`. What it sent still goes out; the
/// socket stays until its close is done.
pub fn close(owner: u64, id: u64) -> Result<(), Error> {
    let mut guard = NET.lock();
    let net = guard.as_mut().ok_or(Error::Unavailable)?;
    let mut streams = STREAMS.lock();
    let at = index(id)
        .filter(|&at| {
            streams
                .open
                .get(at)
                .is_some_and(|s| s.as_ref().is_some_and(|s| s.owner == owner))
        })
        .ok_or(Error::BadHandle)?;
    let Some(stream) = streams.open[at].take() else {
        return Err(Error::BadHandle);
    };
    net.sockets.get_mut::<tcp::Socket>(stream.socket).close();
    if streams.closing.len() == MAX_CLOSING {
        let oldest = streams.closing.remove(0);
        net.sockets.get_mut::<tcp::Socket>(oldest).abort();
        net.sockets.remove(oldest);
    }
    streams.closing.push(stream.socket);
    drop(streams);
    net.poll();
    Ok(())
}

/// Every stream `pid` held is aborted and gone: a process that ends keeps
/// nothing open.
pub fn on_exit(pid: u64) {
    let mut guard = NET.lock();
    let Some(net) = guard.as_mut() else {
        return;
    };
    let mut streams = STREAMS.lock();
    for slot in streams.open.iter_mut() {
        if slot.as_ref().is_some_and(|s| s.owner == pid)
            && let Some(stream) = slot.take()
        {
            net.sockets.get_mut::<tcp::Socket>(stream.socket).abort();
            net.sockets.remove(stream.socket);
        }
    }
    drop(streams);
    net.poll();
}

/// Removes the closed streams whose close is done; called after each poll.
pub(super) fn reap(net: &mut Net) {
    let Some(mut streams) = STREAMS.try_lock() else {
        return;
    };
    streams.closing.retain(|&handle| {
        let socket = net.sockets.get::<tcp::Socket>(handle);
        let done = matches!(socket.state(), tcp::State::Closed | tcp::State::TimeWait);
        if done {
            net.sockets.remove(handle);
        }
        !done
    });
}

/// Streams open now: `owner`'s, and in all.
pub fn count(owner: u64) -> (usize, usize) {
    let streams = STREAMS.lock();
    let open = streams.open.iter().flatten();
    let mine = open.clone().filter(|s| s.owner == owner).count();
    (mine, open.count())
}
