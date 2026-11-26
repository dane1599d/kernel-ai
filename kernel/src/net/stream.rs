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
//! - **A revoked grant** (bug hunt 2's H2-18): a stream connected on a
//!   `NetDest` given under a grant node is under that node too. Revoking it
//!   aborts the stream at once ([`on_revoke`]): the peer gets a reset, the
//!   buffers are freed, and nothing more leaves or arrives. Its handle stays
//!   until closed and answers Revoked to every use; a NET_POLL waiting on it
//!   returns Revoked at its next look. It no longer counts against the limits.
//!
//! Locks: `NET`, then `STREAMS`, never the other way.

use super::{NET, Net, next_port};
use crate::handle::grant::Kernel;
use crate::hpet;
use alloc::vec::Vec;
use captable::grant::{GrantRef, Live};
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
    /// `None` once its grant was revoked ([`on_revoke`]): the socket is
    /// aborted and gone, and the place waits for the handle to close, so its
    /// id never names a later stream meanwhile.
    socket: Option<SocketHandle>,
    bulk: bool,
    /// The day's bytes of the agent `NetDest` it was connected on, if any.
    meter: Option<super::dest::Meter>,
    /// It has been established at least once.
    connected: bool,
    /// The grant node the `NetDest` it was connected on was given under.
    grant: Option<GrantRef>,
}

impl Stream {
    /// It has a socket: it counts against the limits.
    fn open(&self) -> bool {
        self.socket.is_some()
    }

    /// It has a socket and its grant node was revoked: [`on_revoke`] aborts it.
    fn revoked(&self) -> bool {
        self.open() && self.grant.is_some_and(|grant| !Kernel.alive(grant))
    }
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
/// against its `NetDest`; `bulk` gives it the larger buffer, and `meter` is
/// that `NetDest`'s day's bytes, if it is an agent's. `grant` is the node
/// that `NetDest` was given under: Revoked if it is gone. Its id.
pub fn connect(
    owner: u64,
    addr: Ipv4Addr,
    port: u16,
    bulk: bool,
    meter: Option<super::dest::Meter>,
    grant: Option<GrantRef>,
) -> Result<u64, Error> {
    let mut guard = NET.lock();
    let net = guard.as_mut().ok_or(Error::Unavailable)?;
    if net.setup.status().address.is_none() {
        return Err(Error::Unavailable);
    }
    let mut streams = STREAMS.lock();
    // Under this lock, which [`on_revoke`] takes only after the generations
    // moved on: a revoke is either seen here or finds the stream there.
    if grant.is_some_and(|grant| !Kernel.alive(grant)) {
        return Err(Error::Revoked);
    }
    let live = streams.open.iter().flatten().filter(|s| s.open());
    let (mine, bulks, my_bulks) = live.clone().fold((0, 0, 0), |(m, b, mb), s| {
        let own = s.owner == owner;
        (
            m + usize::from(own),
            b + usize::from(s.bulk),
            mb + usize::from(own && s.bulk),
        )
    });
    let total = live.count();
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
        socket: Some(socket),
        bulk,
        meter,
        connected: false,
        grant,
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
    // Every ephemeral port held: as full as the stream limits (H5-19).
    let local = next_port(&net.sockets).ok_or(Error::Busy)?;
    socket
        .connect(net.iface.context(), (IpAddress::Ipv4(addr), port), local)
        .map_err(|_| Error::Invalid)?;
    Ok(net.sockets.add(socket))
}

/// Runs `f` on `owner`'s stream `id` with the interface just polled; Revoked
/// once its grant was ([`on_revoke`]).
fn with<R>(
    owner: u64,
    id: u64,
    f: impl FnOnce(&mut tcp::Socket, &mut bool) -> R,
) -> Result<R, Error> {
    crate::threads::ready::not_in_ready(&"the network");
    let mut guard = NET.lock();
    let net = guard.as_mut().ok_or(Error::Unavailable)?;
    net.poll();
    let mut streams = STREAMS.lock();
    let stream = index(id)
        .and_then(|at| streams.open.get_mut(at)?.as_mut())
        .filter(|s| s.owner == owner)
        .ok_or(Error::BadHandle)?;
    let socket = stream.socket.ok_or(Error::Revoked)?;
    let socket = net.sockets.get_mut::<tcp::Socket>(socket);
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

/// The local port `owner`'s stream `id` was given: `test=net-stream`'s look
/// at port reuse (bug hunt 5's H5-19).
pub fn local_port(owner: u64, id: u64) -> Option<u16> {
    with(owner, id, |socket, _| {
        socket.local_endpoint().map(|end| end.port)
    })
    .ok()
    .flatten()
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
/// socket stays until its close is done. A revoked stream's place is freed.
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
    // A revoked stream's socket went at the revoke.
    let Some(socket) = stream.socket else {
        return Ok(());
    };
    net.sockets.get_mut::<tcp::Socket>(socket).close();
    linger(net, &mut streams.closing, socket);
    drop(streams);
    net.poll();
    Ok(())
}

/// Keeps closed `socket` until [`reap`] finds its close done (its FIN goes
/// out at the next poll); past [`MAX_CLOSING`] the oldest is aborted and
/// dropped.
fn linger(net: &mut Net, closing: &mut Vec<SocketHandle>, socket: SocketHandle) {
    if closing.len() == MAX_CLOSING {
        let oldest = closing.remove(0);
        net.sockets.get_mut::<tcp::Socket>(oldest).abort();
        net.sockets.remove(oldest);
    }
    closing.push(socket);
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
            && let Some(socket) = stream.socket
        {
            net.sockets.get_mut::<tcp::Socket>(socket).abort();
            net.sockets.remove(socket);
        }
    }
    drop(streams);
    net.poll();
}

/// After a grant node was revoked (`handle::grant::revoke`): every stream
/// connected on a `NetDest` given under a dead node is aborted, so the peer
/// gets a reset, its buffers go, and nothing more leaves or arrives. Its place
/// stays, socket-less, until its handle closes, and every use answers Revoked.
pub fn on_revoke() {
    // Most revokes end a request that held no stream: no need for NET then.
    // A stream connected after this look sees the revoke itself (`connect`).
    if !STREAMS.lock().open.iter().flatten().any(Stream::revoked) {
        return;
    }
    let mut guard = NET.lock();
    let Some(net) = guard.as_mut() else {
        return;
    };
    // Not through `linger`: past its 32 a revoke reaching more streams would
    // drop some before their reset went, and cut others' closes short. Only
    // streams with a socket are aborted, and those are never more than the
    // limit.
    let mut aborted = [None; MAX_STREAMS];
    let mut streams = STREAMS.lock();
    let revoked = streams
        .open
        .iter_mut()
        .flatten()
        .filter(|stream| stream.revoked());
    for (place, stream) in aborted.iter_mut().zip(revoked) {
        if let Some(socket) = stream.socket.take() {
            net.sockets.get_mut::<tcp::Socket>(socket).abort();
            *place = Some(socket);
        }
    }
    drop(streams);
    // One poll sends each aborted socket's reset; then the sockets go.
    net.poll();
    for socket in aborted.into_iter().flatten() {
        net.sockets.remove(socket);
    }
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

/// Streams open now, with a socket: `owner`'s, and in all.
pub fn count(owner: u64) -> (usize, usize) {
    let streams = STREAMS.lock();
    let open = streams.open.iter().flatten().filter(|s| s.open());
    let mine = open.clone().filter(|s| s.owner == owner).count();
    (mine, open.count())
}

/// Places of `owner`'s revoked streams whose handles are not closed yet.
pub fn revoked_places(owner: u64) -> usize {
    let streams = STREAMS.lock();
    streams
        .open
        .iter()
        .flatten()
        .filter(|s| s.owner == owner && !s.open())
        .count()
}
