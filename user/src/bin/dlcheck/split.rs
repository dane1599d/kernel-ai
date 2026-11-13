//! Where a download's time goes (the manager on #770): each connection's DNS,
//! TCP and TLS handshake, each request's wait for its head and its body, and
//! kaifs's writes, marks, re-reads and move-in. What is left of the whole is
//! the chunk and whole-target hashes and the copies between them, which
//! `kainet::download` does inline; [`sha256_us`] gives one hash's cost so the
//! rest can be read.
//!
//! The wrappers time what they wrap and change nothing it does, except that
//! [`TimedDial`] resolves the name once more before the dial: the dial's own
//! resolve then comes from the kernel's cache, so `tcp` is the connect.

use alloc::vec;
use core::cell::Cell;
use kainet::download::{DownloadError, Partial};
use kainet::http::{Connect, Dial, HttpError, Url};
use sha2::{Digest, Sha256};
use user::sys;
use user::tls::{NetDial, Tcp};

/// Time in nanoseconds, and counts, over one boot's runs of a download.
#[derive(Default)]
pub struct Split {
    pub connections: Cell<u32>,
    pub dns: Cell<u64>,
    pub tcp: Cell<u64>,
    /// Each connection whole: DNS, TCP and the TLS handshake.
    pub connect: Cell<u64>,
    /// Each fetch of a chunk whole, redirects included.
    pub fetch: Cell<u64>,
    /// From a chunk's first body bytes to its last.
    pub body: Cell<u64>,
    pub writes: Cell<u32>,
    pub write: Cell<u64>,
    pub marks: Cell<u32>,
    /// The mark: read at the start, and each write and commit.
    pub mark: Cell<u64>,
    pub read: Cell<u64>,
    pub finish: Cell<u64>,
}

impl Split {
    /// kaifs's part.
    pub fn kaifs(&self) -> u64 {
        self.write.get() + self.mark.get() + self.read.get() + self.finish.get()
    }

    /// The wait for each head: what a fetch took beyond its connections and
    /// its body.
    pub fn head(&self) -> u64 {
        self.fetch
            .get()
            .saturating_sub(self.connect.get() + self.body.get())
    }

    /// The TLS handshakes: the connections beyond their DNS and TCP.
    pub fn tls(&self) -> u64 {
        self.connect
            .get()
            .saturating_sub(self.dns.get() + self.tcp.get())
    }
}

/// Adds the time since `since` to `cell`, and returns now.
pub fn add(cell: &Cell<u64>, since: u64) -> u64 {
    let now = sys::time_ns();
    cell.set(cell.get() + now.saturating_sub(since));
    now
}

fn count(cell: &Cell<u32>) {
    cell.set(cell.get() + 1);
}

/// Milliseconds, for the lines.
pub fn ms(ns: u64) -> u64 {
    ns / 1_000_000
}

/// [`NetDial`], with its DNS and TCP timed.
pub struct TimedDial<'a> {
    pub inner: NetDial,
    pub split: &'a Split,
}

impl Dial for TimedDial<'_> {
    type Transport = Tcp;

    fn dial(&mut self, host: &str, port: u16) -> Result<Tcp, HttpError> {
        let start = sys::time_ns();
        let mut addrs = [[0u8; 4]; 4];
        // A refusal here is the dial's to report, as it would without this.
        let _ = sys::resolve_on(self.inner.dest, host, &mut addrs);
        let resolved = add(&self.split.dns, start);
        let dialed = self.inner.dial(host, port);
        add(&self.split.tcp, resolved);
        count(&self.split.connections);
        dialed
    }
}

/// A [`Connect`], timed whole.
pub struct TimedConnect<'a, C> {
    pub inner: C,
    pub split: &'a Split,
}

impl<C: Connect> Connect for TimedConnect<'_, C> {
    type Stream = C::Stream;

    fn connect(&mut self, url: &Url) -> Result<C::Stream, HttpError> {
        let start = sys::time_ns();
        let connected = self.inner.connect(url);
        add(&self.split.connect, start);
        connected
    }
}

/// A [`Partial`], each call timed.
pub struct TimedPartial<'a, P> {
    pub inner: &'a mut P,
    pub split: &'a Split,
}

impl<P: Partial> Partial for TimedPartial<'_, P> {
    fn checkpoint(&mut self) -> Result<usize, DownloadError> {
        let start = sys::time_ns();
        let got = self.inner.checkpoint();
        add(&self.split.mark, start);
        got
    }

    fn record(&mut self, done: usize) -> Result<(), DownloadError> {
        let start = sys::time_ns();
        let got = self.inner.record(done);
        add(&self.split.mark, start);
        count(&self.split.marks);
        got
    }

    fn write(&mut self, offset: u64, bytes: &[u8]) -> Result<(), DownloadError> {
        let start = sys::time_ns();
        let got = self.inner.write(offset, bytes);
        add(&self.split.write, start);
        count(&self.split.writes);
        got
    }

    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), DownloadError> {
        let start = sys::time_ns();
        let got = self.inner.read(offset, buf);
        add(&self.split.read, start);
        got
    }

    fn finish(&mut self) -> Result<(), DownloadError> {
        let start = sys::time_ns();
        let got = self.inner.finish();
        add(&self.split.finish, start);
        got
    }

    // A wrapper must pass these on, or the defaults hold: not held alone,
    // and no length, which `download` refuses to move in.
    fn held_alone(&self) -> bool {
        self.inner.held_alone()
    }

    fn size(&mut self) -> Result<u64, DownloadError> {
        let start = sys::time_ns();
        let got = self.inner.size();
        add(&self.split.read, start);
        got
    }
}

/// What one SHA-256 of `len` bytes takes, in microseconds: `download` hashes
/// each chunk twice, for the chunk and for the whole target.
pub fn sha256_us(len: usize) -> u64 {
    let bytes = vec![0x5au8; len];
    let start = sys::time_ns();
    let hash = Sha256::digest(&bytes);
    let took = sys::time_ns().saturating_sub(start);
    // Kept, so the hash is not optimised away.
    core::hint::black_box(hash);
    took / 1_000
}
