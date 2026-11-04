//! A TLS 1.3 connection on rustls's unbuffered API, which needs no std.
//!
//! [`Tls`] holds no transport: the caller feeds it the peer's bytes and sends
//! what it encodes. [`Session`] does both over a blocking [`Transport`], as a
//! TCP stream will be once the kernel has one for ring 3 (P5 task 2). The
//! same code serves the client and, for tests and the mock, the server.

use alloc::vec::Vec;
use rustls::client::{ClientConnectionData, UnbufferedClientConnection};
use rustls::server::{ServerConnectionData, UnbufferedServerConnection};
use rustls::unbuffered::{
    ConnectionState, EncodeError, EncryptError, InsufficientSizeError, UnbufferedStatus,
};

/// The most of the peer's bytes held before rustls takes them: several of
/// TLS's largest records (2^14 bytes of plaintext and 256 of overhead).
pub const MAX_INCOMING: usize = 64 * 1024;

/// Why a connection stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum TlsError {
    /// rustls refused: a certificate, the handshake or a record.
    Tls(rustls::Error),
    /// The transport failed.
    Transport(&'static str),
    /// The peer's bytes ended before the handshake or a close_notify: a
    /// connection cut short, which a caller must not take as a whole answer.
    Truncated,
    /// The peer sent more than [`MAX_INCOMING`] without a whole record.
    TooLarge,
}

impl From<rustls::Error> for TlsError {
    fn from(error: rustls::Error) -> Self {
        TlsError::Tls(error)
    }
}

/// Where [`Tls::advance`] stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// It needs more of the peer's bytes.
    NeedInput,
    /// The handshake is done and nothing waits: data may be written.
    Ready,
    /// The data given was encrypted into the outgoing bytes.
    Wrote,
    /// The peer closed the connection with a close_notify.
    Closed,
}

/// A client or server connection, as [`Tls`] drives it.
pub trait Side {
    type Data;
    fn process<'c, 'i>(
        &'c mut self,
        incoming: &'i mut [u8],
    ) -> UnbufferedStatus<'c, 'i, Self::Data>;
}

impl Side for UnbufferedClientConnection {
    type Data = ClientConnectionData;
    fn process<'c, 'i>(
        &'c mut self,
        incoming: &'i mut [u8],
    ) -> UnbufferedStatus<'c, 'i, Self::Data> {
        self.process_tls_records(incoming)
    }
}

impl Side for UnbufferedServerConnection {
    type Data = ServerConnectionData;
    fn process<'c, 'i>(
        &'c mut self,
        incoming: &'i mut [u8],
    ) -> UnbufferedStatus<'c, 'i, Self::Data> {
        self.process_tls_records(incoming)
    }
}

/// One step's outcome, inside [`Tls::advance`].
enum Step {
    Again,
    Stop(Progress),
}

/// A connection without a transport.
pub struct Tls<C> {
    conn: C,
    incoming: Vec<u8>,
    outgoing: Vec<u8>,
    plain: Vec<u8>,
    read_at: usize,
    /// The peer's close_notify came. rustls says so once, so it is kept.
    peer_closed: bool,
}

impl<C: Side> Tls<C> {
    pub fn new(conn: C) -> Self {
        Self {
            conn,
            incoming: Vec::new(),
            outgoing: Vec::new(),
            plain: Vec::new(),
            read_at: 0,
            peer_closed: false,
        }
    }

    /// The connection, to ask what was negotiated.
    pub fn connection(&self) -> &C {
        &self.conn
    }

    /// The connection itself, for a [`Session`]. Bytes fed or encoded and not
    /// yet taken are dropped, so call it before any.
    pub fn into_connection(self) -> C {
        self.conn
    }

    /// Room left for the peer's bytes.
    pub fn room(&self) -> usize {
        MAX_INCOMING - self.incoming.len()
    }

    /// Takes as many of the peer's `bytes` as there is room for; how many.
    pub fn feed(&mut self, bytes: &[u8]) -> usize {
        let taken = bytes.len().min(self.room());
        self.incoming.extend_from_slice(&bytes[..taken]);
        taken
    }

    /// The bytes to send to the peer, in order; the caller sends them all.
    pub fn take_outgoing(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.outgoing)
    }

    /// Whether bytes wait to be sent.
    pub fn has_outgoing(&self) -> bool {
        !self.outgoing.is_empty()
    }

    /// Decrypted data not yet read.
    pub fn plain(&self) -> &[u8] {
        &self.plain[self.read_at..]
    }

    /// Marks `n` bytes of [`Tls::plain`] read.
    pub fn consume(&mut self, n: usize) {
        self.read_at = (self.read_at + n).min(self.plain.len());
        if self.read_at == self.plain.len() {
            self.plain.clear();
            self.read_at = 0;
        }
    }

    /// Runs rustls on the bytes fed so far, until it needs more of them,
    /// can write, has written `write`, or the peer closed. Decrypted data
    /// collects in [`Tls::plain`], records to send in the outgoing bytes.
    pub fn advance(&mut self, write: Option<&[u8]>) -> Result<Progress, TlsError> {
        if self.peer_closed && write.is_none() {
            return Ok(Progress::Closed);
        }
        loop {
            match self.step(write) {
                Ok(Step::Stop(progress)) => return Ok(progress),
                Ok(Step::Again) => {}
                Err(error) => {
                    self.queue_alert();
                    return Err(error);
                }
            }
        }
    }

    /// After rustls refused something, the alert it queued for the peer, into
    /// the outgoing bytes. rustls hands queued records over before it reads
    /// again, so one more call gives the alert; a second would read the
    /// refused record once more.
    fn queue_alert(&mut self) {
        let Self {
            conn,
            incoming,
            outgoing,
            ..
        } = self;
        let UnbufferedStatus { discard, state } = conn.process(incoming);
        if let Ok(ConnectionState::EncodeTlsData(mut encode)) = state {
            let _ = grow_into(outgoing, |buf| match encode.encode(buf) {
                Ok(n) => Ok(n),
                Err(EncodeError::InsufficientSize(size)) => Err(Grow::By(size)),
                Err(_) => Err(Grow::Fail),
            });
        }
        discard_front(incoming, discard);
    }

    /// Queues a close_notify, once the handshake is done.
    pub fn close(&mut self) -> Result<(), TlsError> {
        let Self {
            conn,
            incoming,
            outgoing,
            ..
        } = self;
        let UnbufferedStatus { discard, state } = conn.process(incoming);
        if let ConnectionState::WriteTraffic(mut traffic) = state? {
            grow_into(outgoing, |buf| match traffic.queue_close_notify(buf) {
                Ok(n) => Ok(n),
                Err(EncryptError::InsufficientSize(size)) => Err(Grow::By(size)),
                Err(_) => Err(Grow::Fail),
            })?;
        }
        discard_front(incoming, discard);
        Ok(())
    }

    fn step(&mut self, write: Option<&[u8]>) -> Result<Step, TlsError> {
        let Self {
            conn,
            incoming,
            outgoing,
            plain,
            peer_closed,
            ..
        } = self;
        let UnbufferedStatus { mut discard, state } = conn.process(incoming);
        let step = match state? {
            ConnectionState::ReadTraffic(mut traffic) => {
                while let Some(record) = traffic.next_record() {
                    let record = record?;
                    discard += record.discard;
                    plain.extend_from_slice(record.payload);
                }
                Step::Again
            }
            ConnectionState::EncodeTlsData(mut encode) => {
                grow_into(outgoing, |buf| match encode.encode(buf) {
                    Ok(n) => Ok(n),
                    Err(EncodeError::InsufficientSize(size)) => Err(Grow::By(size)),
                    Err(_) => Err(Grow::Fail),
                })?;
                Step::Again
            }
            // The bytes stay in `outgoing` in order, and the caller sends them.
            ConnectionState::TransmitTlsData(transmit) => {
                transmit.done();
                Step::Again
            }
            ConnectionState::BlockedHandshake => Step::Stop(Progress::NeedInput),
            ConnectionState::WriteTraffic(mut traffic) => match write {
                Some(data) => {
                    grow_into(outgoing, |buf| match traffic.encrypt(data, buf) {
                        Ok(n) => Ok(n),
                        Err(EncryptError::InsufficientSize(size)) => Err(Grow::By(size)),
                        Err(_) => Err(Grow::Fail),
                    })?;
                    Step::Stop(Progress::Wrote)
                }
                None => Step::Stop(Progress::Ready),
            },
            ConnectionState::PeerClosed | ConnectionState::Closed => {
                *peer_closed = true;
                Step::Stop(Progress::Closed)
            }
            // Early data is never offered, so a server never sees it.
            _ => {
                return Err(TlsError::Tls(rustls::Error::General(
                    "unexpected TLS state".into(),
                )));
            }
        };
        discard_front(incoming, discard);
        Ok(step)
    }
}

enum Grow {
    By(InsufficientSizeError),
    Fail,
}

/// Appends what `write` writes to `out`, growing `out` as rustls asks.
fn grow_into(
    out: &mut Vec<u8>,
    mut write: impl FnMut(&mut [u8]) -> Result<usize, Grow>,
) -> Result<(), TlsError> {
    let start = out.len();
    let mut room = 1024;
    loop {
        out.resize(start + room, 0);
        match write(&mut out[start..]) {
            Ok(n) => {
                out.truncate(start + n);
                return Ok(());
            }
            Err(Grow::By(size)) if size.required_size > room => room = size.required_size,
            Err(_) => {
                out.truncate(start);
                return Err(TlsError::Tls(rustls::Error::EncryptError));
            }
        }
    }
}

fn discard_front(incoming: &mut Vec<u8>, discard: usize) {
    if discard > 0 {
        incoming.drain(..discard.min(incoming.len()));
    }
}

/// A blocking byte stream to the peer.
pub trait Transport {
    /// Sends all of `bytes`.
    fn send(&mut self, bytes: &[u8]) -> Result<(), &'static str>;
    /// Receives some bytes into `buf`; 0 when the peer closed.
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, &'static str>;
}

/// A connection over a blocking transport.
pub struct Session<C, T> {
    tls: Tls<C>,
    io: T,
    buf: Vec<u8>,
}

/// What a session reads from the transport at a time.
const RECV_CHUNK: usize = 16 * 1024;

impl<C: Side, T: Transport> Session<C, T> {
    pub fn new(conn: C, io: T) -> Self {
        Self {
            tls: Tls::new(conn),
            io,
            buf: alloc::vec![0u8; RECV_CHUNK],
        }
    }

    pub fn tls(&self) -> &Tls<C> {
        &self.tls
    }

    /// Runs the handshake to its end.
    pub fn handshake(&mut self) -> Result<(), TlsError> {
        loop {
            let progress = self.advance(None)?;
            match progress {
                Progress::Ready | Progress::Wrote => return Ok(()),
                Progress::NeedInput => self.fill()?,
                Progress::Closed => return Err(TlsError::Truncated),
            }
        }
    }

    /// Encrypts and sends all of `data`.
    pub fn write_all(&mut self, data: &[u8]) -> Result<(), TlsError> {
        loop {
            let progress = self.advance(Some(data))?;
            match progress {
                Progress::Wrote => return Ok(()),
                Progress::Ready => {}
                Progress::NeedInput => self.fill()?,
                Progress::Closed => return Err(TlsError::Truncated),
            }
        }
    }

    /// Reads decrypted data into `out`: how much, or 0 once the peer closed
    /// with a close_notify.
    pub fn read(&mut self, out: &mut [u8]) -> Result<usize, TlsError> {
        loop {
            let ready = self.tls.plain();
            if !ready.is_empty() {
                let n = ready.len().min(out.len());
                out[..n].copy_from_slice(&ready[..n]);
                self.tls.consume(n);
                return Ok(n);
            }
            let progress = self.advance(None)?;
            match progress {
                Progress::Closed if self.tls.plain().is_empty() => return Ok(0),
                Progress::Closed => {}
                Progress::Ready | Progress::Wrote | Progress::NeedInput => {
                    if self.tls.plain().is_empty() {
                        self.fill()?;
                    }
                }
            }
        }
    }

    /// Sends a close_notify.
    pub fn close(&mut self) -> Result<(), TlsError> {
        self.tls.close()?;
        self.flush()
    }

    /// [`Tls::advance`], then sends what it encoded, an alert included when
    /// it failed: the TLS error comes first.
    fn advance(&mut self, write: Option<&[u8]>) -> Result<Progress, TlsError> {
        let progress = self.tls.advance(write);
        let flushed = self.flush();
        let progress = progress?;
        flushed?;
        Ok(progress)
    }

    fn flush(&mut self) -> Result<(), TlsError> {
        if self.tls.has_outgoing() {
            let out = self.tls.take_outgoing();
            self.io.send(&out).map_err(TlsError::Transport)?;
        }
        Ok(())
    }

    fn fill(&mut self) -> Result<(), TlsError> {
        let room = self.tls.room().min(self.buf.len());
        if room == 0 {
            return Err(TlsError::TooLarge);
        }
        let n = self
            .io
            .recv(&mut self.buf[..room])
            .map_err(TlsError::Transport)?;
        if n == 0 {
            return Err(TlsError::Truncated);
        }
        self.tls.feed(&self.buf[..n]);
        Ok(())
    }
}
