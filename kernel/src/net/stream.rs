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
//! - **A process that ends** has every stream it held open aborted
//!   ([`on_exit`]).
//! - **A closed stream** keeps its socket, and its buffers, until its close is
//!   done ([`reap`]), so it counts against every limit until then. A peer that
//!   never sends its FIN cannot keep it: [`CLOSE_DEADLINE_MS`] after the close
//!   the stream is aborted (bug hunt #13).
//!
//! Locks: `NET`, then `STREAMS`, never the other way.

use super::{NET, Net, next_port};
use crate::hpet;
use alloc::vec::Vec;
use core::net::Ipv4Addr;
use kabi::Error;
use netcore::streams::{Held, Limits, Linger};
use smoltcp::iface::SocketHandle;
use smoltcp::socket::tcp;
use smoltcp::wire::IpAddress;
use spin::Mutex;

/// Streams one process may hold, open or closing.
pub const MAX_PER_PROCESS: usize = kabi::net::STREAMS_PER_PROCESS;
/// Streams at once, open or closing, in all.
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
const LIMITS: Limits = Limits {
    per_process: MAX_PER_PROCESS,
    total: MAX_STREAMS,
    bulk: MAX_BULK,
    bulk_per_process: MAX_BULK_PER_PROCESS,
};
/// The most heap the streams' buffers can take. A closing stream counts
/// against the same limits as an open one, so the slots bound both: 32 x
/// (256 KiB + 64 KiB) = 10 MiB, plus 4 bulk ones at 1 MiB - 256 KiB = 768
/// KiB more each = 3 MiB, 13 MiB in all, under half the 32 MiB heap. Before
/// bug hunt #13 it was 64 slots (23 MiB) with up to 32 closing bulk streams
/// (34 MiB) on top.
const WORST_CASE_BYTES: usize = LIMITS.worst_case(RX_BUFFER, BULK_RX_BUFFER, TX_BUFFER);
const _: () = assert!(WORST_CASE_BYTES <= crate::memory::heap::HEAP_SIZE as usize / 2);
/// How long a closed stream may take to close before it is aborted: FIN-WAIT,
/// CLOSING and LAST-ACK alike. smoltcp's own timeout cannot be the bound: it
/// counts from the peer's last segment, so a peer that keeps answering (an
/// ACK to each keep-alive, a window update) is never timed out, and a stream
/// idle for long before its close would time out before its FIN went out.
pub const CLOSE_DEADLINE_MS: u64 = kabi::net::STREAM_CLOSE_MS;
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
    /// The day's bytes of the agent `NetDest` it was connected on, if any.
    meter: Option<super::dest::Meter>,
    /// It has been established at least once.
    connected: bool,
}

/// A closed stream whose close is not done yet.
struct Closing {
    owner: u64,
    socket: SocketHandle,
    bulk: bool,
    /// When it is aborted if its close is not done by then (`hpet` ns).
    deadline: u64,
}

struct Streams {
    open: Vec<Option<Stream>>,
    closing: Vec<Closing>,
}

impl Streams {
    /// Every stream the limits count: open, and closed but not done.
    fn held(&self) -> impl Iterator<Item = Held> + '_ {
        let open = self.open.iter().flatten().map(|s| (s.owner, s.bulk));
        let closing = self.closing.iter().map(|c| (c.owner, c.bulk));
        open.chain(closing)
            .map(|(owner, bulk)| Held { owner, bulk })
    }
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
/// against its `NetDest`; `bulk` gives it the larger buffer, and `meter` is
/// that `NetDest`'s day's bytes, if it is an agent's. Its id.
pub fn connect(
    owner: u64,
    addr: Ipv4Addr,
    port: u16,
    bulk: bool,
    meter: Option<super::dest::Meter>,
) -> Result<u64, Error> {
    let mut guard = NET.lock();
    let net = guard.as_mut().ok_or(Error::Unavailable)?;
    if net.setup.status().address.is_none() {
        return Err(Error::Unavailable);
    }
    let mut streams = STREAMS.lock();
    // Closes done since the last poll free their slots first.
    sweep(net, &mut streams, hpet::nanos());
    if !LIMITS.allow(streams.held(), owner, bulk) {
        return Err(Error::Busy);
    }
    // Room for every stream to be closing, so a close never allocates.
    let room = MAX_STREAMS.saturating_sub(streams.closing.len());
    streams
        .closing
        .try_reserve_exact(room)
        .map_err(|_| Error::NoMemory)?;
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
        meter,
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

/// The day's bytes stream `id` of `owner` counts on, if it is an agent's.
pub fn meter_of(owner: u64, id: u64) -> Option<super::dest::Meter> {
    let streams = STREAMS.lock();
    let stream = streams.open.get(index(id)?)?.as_ref()?;
    (stream.owner == owner).then(|| stream.meter.clone())?
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
        let ticket = super::frames_ticket();
        let bits = with(owner, id, |socket, connected| state(socket, *connected))?;
        if bits & (want | ready::FAILED) != 0 || hpet::nanos() >= deadline {
            return Ok(bits);
        }
        if cancel() {
            return Err(Error::Interrupted);
        }
        super::wait_for_frames(ticket, POLL_GAP_MS);
    }
}

/// NET_CLOSE: closes `owner`'s stream `id`. What it sent still goes out; the
/// socket stays, and counts against the limits, until its close is done or
/// [`CLOSE_DEADLINE_MS`] has passed.
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
    // Within the room connect reserved: the limits keep open and closing
    // streams together to MAX_STREAMS.
    streams.closing.push(Closing {
        owner: stream.owner,
        socket: stream.socket,
        bulk: stream.bulk,
        deadline: netcore::streams::deadline(hpet::nanos(), CLOSE_DEADLINE_MS),
    });
    drop(streams);
    net.poll();
    Ok(())
}

/// Every stream `pid` held open is aborted and gone: a process that ends
/// keeps nothing open. The ones it closed still finish closing, within their
/// deadline.
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

/// Removes the closed streams whose close is done, and aborts the ones past
/// their deadline; called after each poll.
pub(super) fn reap(net: &mut Net) {
    let Some(mut streams) = STREAMS.try_lock() else {
        return;
    };
    sweep(net, &mut streams, hpet::nanos());
}

/// [`reap`] with the table held. An aborted socket stays for one more poll,
/// which sends the peer its reset; it is CLOSED then, and removed.
fn sweep(net: &mut Net, streams: &mut Streams, now: u64) {
    streams.closing.retain(|closing| {
        let socket = net.sockets.get_mut::<tcp::Socket>(closing.socket);
        let done = matches!(socket.state(), tcp::State::Closed | tcp::State::TimeWait);
        match netcore::streams::linger(done, closing.deadline, now) {
            Linger::Wait => true,
            Linger::Abort => {
                socket.abort();
                true
            }
            Linger::Done => {
                net.sockets.remove(closing.socket);
                false
            }
        }
    });
}

/// Streams open now: `owner`'s, and in all.
pub fn count(owner: u64) -> (usize, usize) {
    let streams = STREAMS.lock();
    let open = streams.open.iter().flatten();
    let mine = open.clone().filter(|s| s.owner == owner).count();
    (mine, open.count())
}

/// Closed streams whose close is not done yet, in all.
pub fn closing() -> usize {
    STREAMS.lock().closing.len()
}
