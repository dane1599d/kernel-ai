//! What a ring-3 TLS client needs from the OS, for `kainet` (P5 task 12):
//! TCP streams on a `NetDest` handle as its transport, the roots it trusts,
//! its randomness and its clock. `fetch`, `pkg`, `updated` and the model
//! download share these, so no program carries a copy of its own.
//!
//! - **Transport** ([`NetDial`], [`Tcp`]): a host is resolved on the handle
//!   with RESOLVE_ON, which the kernel answers only for names under the
//!   handle's suffixes, and connected with NET_CONNECT, which reaches only an
//!   address the handle resolved. A name the handle does not cover is
//!   `HttpError::Refused`. Each wait is at most [`IO_TIMEOUT_MS`], and never
//!   past the dial's deadline (K's review of #755), so a stalled or trickling
//!   peer ends the fetch rather than hold it.
//! - **Roots** ([`roots`]): Mozilla's, and the boot bundle's
//!   `netroots/<n>.der`, which only a test bundle has (xtask's HTTPS mock's
//!   CA).
//! - **Randomness** ([`RANDOM`]): ENTROPY, the kernel's seeded generator.
//! - **Clock** ([`wall`]): NET_TIME, refused before `kainet`'s floor date.

use crate::sys;
use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec;
use kabi::Error;
use kabi::handle::Handle;
use kabi::net::{
    MAX_ADDRESSES, MAX_NETROOTS, NETROOTS, STREAM_BULK, STREAM_CLOSED, STREAM_CONNECTED,
    STREAM_FAILED, STREAM_MAX_CALL, STREAM_READABLE, STREAM_WRITABLE,
};
use kainet::client::{self, FLOOR_S, Wall};
use kainet::http::{Dial, HttpError};
use kainet::tls::Transport;
use rustls::crypto::{GetRandomFailed, SecureRandom};
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};

/// The longest a send or a receive waits for the peer.
pub const IO_TIMEOUT_MS: u64 = 20_000;
/// The longest a connection takes to open.
pub const CONNECT_TIMEOUT_MS: u64 = 10_000;
/// The largest extra root read from the boot bundle.
const MAX_ROOT: usize = 16 * 1024;
/// ENTROPY's most bytes a call.
const MAX_ENTROPY: usize = 256;

/// Randomness from ENTROPY.
#[derive(Debug)]
pub struct Entropy;

impl SecureRandom for Entropy {
    fn fill(&self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        for part in buf.chunks_mut(MAX_ENTROPY) {
            match sys::entropy(part) {
                Ok(n) if n == part.len() => {}
                _ => return Err(GetRandomFailed),
            }
        }
        Ok(())
    }
}

/// The randomness every `kainet` client here uses.
pub static RANDOM: Entropy = Entropy;

fn now_s() -> Option<u64> {
    sys::net_time().ok()
}

/// The clock: NET_TIME, refused before [`FLOOR_S`].
pub fn wall() -> Wall {
    Wall {
        now_s,
        floor_s: FLOOR_S,
    }
}

/// A boot bundle file, if this process may read it and it is at most
/// [`MAX_ROOT`] bytes.
fn boot_file(path: &str) -> Option<alloc::vec::Vec<u8>> {
    // A call with no room answers the length and copies nothing.
    let len = sys::boot_read(path, &mut []).ok()?;
    if len == 0 || len > MAX_ROOT {
        return None;
    }
    let mut bytes = vec![0u8; len];
    (sys::boot_read(path, &mut bytes).ok()? == len).then_some(bytes)
}

/// Mozilla's roots, and the boot bundle's `netroots/1.der` onwards.
pub fn roots() -> RootCertStore {
    let mut roots = client::mozilla_roots();
    for n in 1..=MAX_NETROOTS {
        let Some(der) = boot_file(&format!("{NETROOTS}{n}.der")) else {
            break;
        };
        if roots.add(CertificateDer::from(der)).is_err() {
            crate::println!("tls: {NETROOTS}{n}.der is not a root certificate; left out");
        }
    }
    roots
}

/// TLS 1.3 on our provider, with [`roots`], [`RANDOM`] and [`wall`].
pub fn config() -> Result<Arc<ClientConfig>, rustls::Error> {
    let provider = Arc::new(kainet::provider::provider(&RANDOM));
    client::config(provider, roots(), Arc::new(wall()))
}

/// How long a wait may take: `most` milliseconds, and never past
/// `deadline_ns` on `sys::time_ns`; `None` once the deadline has passed.
fn wait_ms(most: u64, deadline_ns: u64) -> Option<u64> {
    let left_ms = deadline_ns.saturating_sub(sys::time_ns()) / 1_000_000;
    (left_ms > 0).then_some(most.min(left_ms))
}

/// A TCP stream, closed when dropped.
pub struct Tcp {
    stream: Handle,
    /// When the call it serves must end, on `sys::time_ns`.
    deadline_ns: u64,
}

impl Tcp {
    fn wait(&self, want: u64) -> Result<(), &'static str> {
        self.wait_at_most(want, IO_TIMEOUT_MS)
    }

    fn wait_at_most(&self, want: u64, most: u64) -> Result<(), &'static str> {
        let ms = wait_ms(most, self.deadline_ns).ok_or("the call's time ran out")?;
        let state = sys::net_poll(self.stream, want, ms).map_err(|_| "poll failed")?;
        if state & STREAM_FAILED != 0 {
            return Err("the connection failed");
        }
        match state & want {
            0 => Err("the peer did not answer in time"),
            _ => Ok(()),
        }
    }
}

impl Drop for Tcp {
    fn drop(&mut self) {
        let _ = sys::net_close(self.stream);
    }
}

impl Transport for Tcp {
    fn send(&mut self, mut bytes: &[u8]) -> Result<(), &'static str> {
        while !bytes.is_empty() {
            let part = &bytes[..bytes.len().min(STREAM_MAX_CALL)];
            match sys::net_send(self.stream, part) {
                Ok(0) => self.wait(STREAM_WRITABLE)?,
                Ok(n) => bytes = &bytes[n.min(bytes.len())..],
                Err(_) => return Err("send failed"),
            }
        }
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, &'static str> {
        let cap = buf.len().min(STREAM_MAX_CALL);
        if cap == 0 {
            return Ok(0);
        }
        loop {
            match sys::net_recv(self.stream, &mut buf[..cap]) {
                Ok(n) => return Ok(n),
                Err(Error::Busy) => self.wait(STREAM_READABLE | STREAM_CLOSED)?,
                Err(_) => return Err("receive failed"),
            }
        }
    }
}

/// Dials on the `NetDest` handle `dest`, which stays the caller's.
pub struct NetDial {
    pub dest: Handle,
    /// Ask for a bulk stream's larger receive buffer (a download).
    pub bulk: bool,
    /// When the call must end, on `sys::time_ns`: no connect, send or
    /// receive waits past it. `u64::MAX` for none.
    pub deadline_ns: u64,
}

impl Dial for NetDial {
    type Transport = Tcp;

    fn dial(&mut self, host: &str, port: u16) -> Result<Tcp, HttpError> {
        let mut addrs = [[0u8; 4]; MAX_ADDRESSES];
        let found = match sys::resolve_on(self.dest, host, &mut addrs) {
            Ok(found) => found.min(MAX_ADDRESSES),
            Err(Error::Permission) => return Err(HttpError::Refused(host.to_string())),
            Err(_) => return Err(HttpError::Connect("the host's name did not resolve")),
        };
        let flags = if self.bulk { STREAM_BULK } else { 0 };
        let mut why = "the host's name has no address";
        for addr in &addrs[..found] {
            let stream = match sys::net_connect(self.dest, *addr, port, flags) {
                Ok(stream) => stream,
                Err(Error::Permission) => return Err(HttpError::Refused(host.to_string())),
                Err(_) => {
                    why = "the connection could not be opened";
                    continue;
                }
            };
            let tcp = Tcp {
                stream,
                deadline_ns: self.deadline_ns,
            };
            match tcp.wait_at_most(STREAM_CONNECTED, CONNECT_TIMEOUT_MS) {
                Ok(()) => return Ok(tcp),
                Err(_) => why = "the host did not accept the connection",
            }
        }
        Err(HttpError::Connect(why))
    }
}
