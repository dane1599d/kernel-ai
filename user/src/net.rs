//! The network for ring-3 TLS clients (plan p5-networking, tasks 8-10 and 12):
//! `kainet`'s TLS 1.3 and HTTP over a process's own `NetDest` handle.
//!
//! - [`Stream`] is a connected TCP stream (NET_CONNECT) as `kainet` reads and
//!   writes one, waiting with NET_POLL and never longer than [`WAIT_MS`] at a
//!   time, so a silent server is a failed read, not a hung program.
//! - [`NetDial`] opens one for a host name: the name is resolved on the handle
//!   (RESOLVE_ON, which answers only names under the handle's suffixes), and each
//!   address is tried in turn. The kernel enforces the handle on the connect, so
//!   a name or address the handle does not allow never gets a stream.
//! - [`Entropy`] is rustls's random source, from the kernel's generator
//!   (ENTROPY), and [`tls_config`] is the client configuration for a set of
//!   roots, with the wall clock the caller gives.
//!
//! These build for the hard-float program class (plan section 4, item 3), where
//! AES-NI and the vector paths can run.

use alloc::sync::Arc;
use kabi::Error;
use kabi::handle::Handle;
use kabi::net::{
    MAX_ADDRESSES, STREAM_CLOSED, STREAM_CONNECTED, STREAM_FAILED, STREAM_MAX_CALL,
    STREAM_READABLE, STREAM_WRITABLE,
};
use kainet::client::{self, Wall};
use kainet::http::{Dial, HttpError};
use kainet::provider;
use kainet::tls::Transport;
use rustls::crypto::{GetRandomFailed, SecureRandom};
use rustls::{ClientConfig, RootCertStore};

use crate::sys;

/// The longest one wait on a stream may take: a connect, room to send, or
/// something to read. Past it the read or write fails.
pub const WAIT_MS: u64 = 15_000;

/// A connected TCP stream. Dropping it closes it.
pub struct Stream {
    handle: Handle,
}

impl Stream {
    /// A stream to `addr`:`port` on `dest`, connected, or why it is not.
    pub fn connect(dest: Handle, addr: [u8; 4], port: u16) -> Result<Self, &'static str> {
        let handle = sys::net_connect(dest, addr, port, 0).map_err(|e| match e {
            Error::Permission => "the handle does not allow that address and port",
            Error::Busy => "too many streams",
            _ => "cannot connect",
        })?;
        // From here the handle is closed on every path.
        let stream = Self { handle };
        let state = sys::net_poll(handle, STREAM_CONNECTED, WAIT_MS)
            .map_err(|_| "the connect was interrupted")?;
        match state & STREAM_CONNECTED != 0 && state & STREAM_FAILED == 0 {
            true => Ok(stream),
            false => Err("did not connect"),
        }
    }

    /// Waits until the stream is any of `want` or has failed; its state.
    fn wait(&self, want: u64) -> Result<u64, &'static str> {
        sys::net_poll(self.handle, want, WAIT_MS).map_err(|_| "the wait was interrupted")
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = sys::net_close(self.handle);
    }
}

impl Transport for Stream {
    fn send(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        let mut sent = 0;
        while sent < bytes.len() {
            let end = bytes.len().min(sent + STREAM_MAX_CALL);
            match sys::net_send(self.handle, &bytes[sent..end]) {
                Ok(n) if n > 0 => sent += n,
                // The send buffer is full: wait for room, or for the stream to fail.
                Ok(_) | Err(Error::Busy) => {
                    let state = self.wait(STREAM_WRITABLE)?;
                    if state & (STREAM_FAILED | STREAM_CLOSED) != 0 {
                        return Err("the stream closed while sending");
                    }
                    if state & STREAM_WRITABLE == 0 {
                        return Err("the stream took nothing in time");
                    }
                }
                Err(Error::Dead) => return Err("the stream closed while sending"),
                Err(_) => return Err("the stream cannot send"),
            }
        }
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, &'static str> {
        let cap = STREAM_MAX_CALL.min(buf.len());
        let buf = &mut buf[..cap];
        loop {
            match sys::net_recv(self.handle, buf) {
                // 0 is the peer's close, with everything it sent read.
                Ok(n) => return Ok(n),
                Err(Error::Busy) => {
                    let state = self.wait(STREAM_READABLE | STREAM_CLOSED)?;
                    if state & (STREAM_READABLE | STREAM_CLOSED) == 0 {
                        return Err(match state & STREAM_FAILED != 0 {
                            true => "the stream failed",
                            false => "nothing came in time",
                        });
                    }
                }
                Err(Error::Dead) => return Err("the stream failed"),
                Err(_) => return Err("the stream cannot receive"),
            }
        }
    }
}

/// Opens streams by name on a `NetDest` handle.
pub struct NetDial {
    /// The handle every resolve and connect is made on.
    pub dest: Handle,
    /// The port a connection to `443` really reaches: 443, but for the boot
    /// cases, whose mock listens where the host gave it room. The handle must
    /// allow it, as it must allow any port.
    pub https_port: u16,
}

impl NetDial {
    pub const fn new(dest: Handle) -> Self {
        Self {
            dest,
            https_port: 443,
        }
    }
}

impl Dial for NetDial {
    type Transport = Stream;

    fn dial(&mut self, host: &str, port: u16) -> Result<Stream, HttpError> {
        let mut addresses = [[0u8; 4]; MAX_ADDRESSES];
        let count = sys::resolve_on(self.dest, host, &mut addresses)
            .map_err(|_| HttpError::Connect("the name does not resolve on this handle"))?;
        let port = if port == 443 { self.https_port } else { port };
        let mut failed = "the name has no address";
        for address in &addresses[..count.min(MAX_ADDRESSES)] {
            match Stream::connect(self.dest, *address, port) {
                Ok(stream) => return Ok(stream),
                Err(why) => failed = why,
            }
        }
        Err(HttpError::Connect(failed))
    }
}

/// rustls's random numbers, from the kernel's generator. A kernel without
/// entropy gives none, and rustls then refuses to make a key.
#[derive(Debug)]
pub struct Entropy;

impl SecureRandom for Entropy {
    fn fill(&self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        for chunk in buf.chunks_mut(kabi::call::ENTROPY_MAX) {
            match sys::entropy(chunk) {
                Ok(n) if n == chunk.len() => {}
                _ => return Err(GetRandomFailed),
            }
        }
        Ok(())
    }
}

static ENTROPY: Entropy = Entropy;

/// The TLS 1.3 client configuration for `roots`, on our provider and the wall
/// clock `now_s` gives (seconds since 1970), refused before [`client::FLOOR_S`].
/// Make it once: the provider's key-exchange groups live as long as the program.
pub fn tls_config(
    roots: RootCertStore,
    now_s: fn() -> Option<u64>,
) -> Result<Arc<ClientConfig>, rustls::Error> {
    let provider = Arc::new(provider::provider(&ENTROPY));
    let wall = Wall {
        now_s,
        floor_s: client::FLOOR_S,
    };
    client::config(provider, roots, Arc::new(wall))
}

/// A root store of the one certificate `der`, for the boot cases' test CA.
pub fn roots_of(der: &[u8]) -> Result<RootCertStore, rustls::Error> {
    let mut roots = RootCertStore::empty();
    roots.add(der.to_vec().into())?;
    Ok(roots)
}
