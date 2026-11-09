//! HTTP/1.1 for P5 (plan p5-networking, task 5): GET and HEAD, a byte range,
//! a body by length, by chunks or until a close_notify, at most five
//! redirects, and limits on size and time. No cookies and no compression;
//! one request a connection. Every redirect's host is checked with the
//! caller's `allow` before it is reached, as the caller's `NetDest` will be
//! by the kernel (task 1): GitHub's release redirect to its assets host is
//! followed only because the caller names that host.

mod body;
pub mod head;
pub mod url;

use crate::tls::{Session, Side, TlsError, Transport};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
pub use head::{ContentRange, Framing, Head, Method, Range};
use rustls::ClientConfig;
use rustls::client::UnbufferedClientConnection;
use rustls::pki_types::ServerName;
pub use url::Url;

/// Why a fetch stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum HttpError {
    Url(&'static str),
    /// No connection to the host.
    Connect(&'static str),
    Tls(TlsError),
    /// The response head is not one this client takes.
    Head(&'static str),
    /// The body's framing is broken.
    Body(&'static str),
    /// The body ended before its length or its last chunk, or the connection
    /// was cut without a close_notify.
    Truncated,
    /// The head or the body is over its limit.
    TooLarge,
    TooManyRedirects,
    /// A redirect to a host the caller may not reach.
    Refused(String),
    /// The fetch's deadline passed.
    Timeout,
    /// A range was asked for and the server sent the whole target (200):
    /// passing it on as the range would put the target's first bytes where
    /// the range's belong.
    RangeIgnored,
}

impl From<TlsError> for HttpError {
    fn from(error: TlsError) -> Self {
        match error {
            TlsError::Truncated => HttpError::Truncated,
            error => HttpError::Tls(error),
        }
    }
}

/// A connection's byte stream: `recv` gives 0 once the peer has ended it
/// cleanly. The fetch's deadline is checked between reads, so a `recv` that
/// blocks must time out on its own: the transport's job (a TCP stream's
/// timeout in the OS, a socket timeout on the host).
pub trait Stream {
    fn send(&mut self, bytes: &[u8]) -> Result<(), HttpError>;
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, HttpError>;
}

impl<C: Side, T: Transport> Stream for Session<C, T> {
    fn send(&mut self, bytes: &[u8]) -> Result<(), HttpError> {
        Ok(self.write_all(bytes)?)
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, HttpError> {
        Ok(self.read(buf)?)
    }
}

/// Opens a connection to a URL's host.
pub trait Connect {
    type Stream: Stream;
    fn connect(&mut self, url: &Url) -> Result<Self::Stream, HttpError>;
}

/// A transport to a host and port: TCP on a `NetDest` in the OS (task 2).
/// Its `recv` times out rather than block for ever (see [`Stream`]).
pub trait Dial {
    type Transport: Transport;
    fn dial(&mut self, host: &str, port: u16) -> Result<Self::Transport, HttpError>;
}

/// TLS 1.3 over what `dial` opens, checked against `config`'s roots for
/// the URL's host.
pub struct TlsConnect<D> {
    pub config: Arc<ClientConfig>,
    pub dial: D,
}

impl<D: Dial> Connect for TlsConnect<D> {
    type Stream = Session<UnbufferedClientConnection, D::Transport>;

    fn connect(&mut self, url: &Url) -> Result<Self::Stream, HttpError> {
        let name = ServerName::try_from(url.host.clone())
            .map_err(|_| HttpError::Url("not a server name"))?;
        let transport = self.dial.dial(&url.host, url.port)?;
        let conn = UnbufferedClientConnection::new(Arc::clone(&self.config), name)
            .map_err(|e| HttpError::Tls(TlsError::Tls(e)))?;
        let mut session = Session::new(conn, transport);
        session.handshake()?;
        Ok(session)
    }
}

/// A fetch's bounds.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The longest response head.
    pub max_head: usize,
    /// The longest body passed on.
    pub max_body: u64,
    /// The most redirects followed.
    pub max_redirects: u8,
    /// When the whole fetch must have ended, on the caller's clock.
    pub deadline_ns: u64,
}

impl Limits {
    /// 16 KiB of head, `max_body`, five redirects, and `deadline_ns`.
    pub fn new(max_body: u64, deadline_ns: u64) -> Self {
        Self {
            max_head: 16 * 1024,
            max_body,
            max_redirects: 5,
            deadline_ns,
        }
    }
}

/// What a fetch came to.
#[derive(Debug, Clone, PartialEq)]
pub struct Fetched {
    /// The URL that answered, after any redirects.
    pub url: Url,
    pub head: Head,
    /// Body bytes passed to the sink.
    pub received: u64,
    /// Redirects followed to get here.
    pub redirects: u8,
    /// Where a redirect [`Fetch::run_within`] handed back leads, made
    /// absolute; `head` is that redirect's.
    pub unfollowed: Option<Url>,
}

/// The most 1xx interim heads skipped before the final one.
const MAX_INTERIM: usize = 4;

/// The statuses that redirect.
fn redirects(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// What a fetch needs besides its request.
pub struct Fetch<'a> {
    pub limits: Limits,
    /// Whether the caller may reach a host on a port.
    pub allow: &'a dyn Fn(&str, u16) -> bool,
    /// The caller's clock, in nanoseconds.
    pub clock: &'a dyn Fn() -> u64,
}

impl Fetch<'_> {
    /// `method` on `url`, for `range`, following redirects. A 200 or 206
    /// body goes to `sink` as it arrives; any other status comes back with
    /// its head and no body read. A 206 must be for the range asked, and a
    /// 200 to a range that does not start at byte 0 or ends early is
    /// [`HttpError::RangeIgnored`], with nothing passed on.
    pub fn run(
        &self,
        connect: &mut impl Connect,
        method: Method,
        url: &Url,
        range: Option<Range>,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>,
    ) -> Result<Fetched, HttpError> {
        self.walk(connect, method, url, range, sink, false)
    }

    /// As [`Fetch::run`], but a redirect to a host `allow` refuses is not
    /// followed: it comes back as the 3xx, with no body read and
    /// [`Fetched::unfollowed`] set, for the caller to decide (`fetch`, P5
    /// task 12: its caller asks about the new host and calls again with a
    /// handle for it). The caller takes that redirect as a hop, so with none
    /// of `max_redirects` left it is [`HttpError::TooManyRedirects`]. The first
    /// URL's host must still be allowed, and redirects `allow` takes are
    /// followed as [`Fetch::run`] follows them.
    pub fn run_within(
        &self,
        connect: &mut impl Connect,
        method: Method,
        url: &Url,
        range: Option<Range>,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>,
    ) -> Result<Fetched, HttpError> {
        self.walk(connect, method, url, range, sink, true)
    }

    fn walk(
        &self,
        connect: &mut impl Connect,
        method: Method,
        url: &Url,
        range: Option<Range>,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>,
        within: bool,
    ) -> Result<Fetched, HttpError> {
        let mut url = url.clone();
        if !(self.allow)(&url.host, url.port) {
            return Err(HttpError::Refused(url.host));
        }
        for followed in 0..=self.limits.max_redirects {
            self.in_time()?;
            let mut stream = connect.connect(&url)?;
            stream.send(&head::request(method, &url, range))?;
            let mut buf = Vec::new();
            let head = self.read_head(&mut stream, method, &mut buf)?;
            if redirects(head.status) {
                let location = head
                    .location
                    .as_deref()
                    .ok_or(HttpError::Head("a redirect with no Location"))?;
                let next = url.join(location).map_err(HttpError::Url)?;
                if !(self.allow)(&next.host, next.port) {
                    if !within {
                        return Err(HttpError::Refused(next.host));
                    }
                    // The caller follows it as a hop of its own: none left.
                    if followed == self.limits.max_redirects {
                        return Err(HttpError::TooManyRedirects);
                    }
                    return Ok(Fetched {
                        url,
                        head,
                        received: 0,
                        redirects: followed,
                        unfollowed: Some(next),
                    });
                }
                url = next;
                continue;
            }
            if !matches!(head.status, 200 | 206) {
                return Ok(Fetched {
                    url,
                    head,
                    received: 0,
                    redirects: followed,
                    unfollowed: None,
                });
            }
            match head.status {
                206 => check_range(range, head.content_range)?,
                // The whole target stands for the range only when the range
                // is the whole target.
                _ if range.is_some_and(|r| r.first > 0 || r.last.is_some()) => {
                    return Err(HttpError::RangeIgnored);
                }
                _ => {}
            }
            let received = self.read_body(&mut stream, head.framing, buf, sink)?;
            return Ok(Fetched {
                url,
                head,
                received,
                redirects: followed,
                unfollowed: None,
            });
        }
        Err(HttpError::TooManyRedirects)
    }

    fn in_time(&self) -> Result<(), HttpError> {
        match (self.clock)() > self.limits.deadline_ns {
            true => Err(HttpError::Timeout),
            false => Ok(()),
        }
    }

    /// Reads to the end of the final head, past any 1xx interim ones (100
    /// Continue, 103 Early Hints); the head, with any body bytes that came
    /// with it left in `buf`. A 101 is refused: nothing here upgrades.
    fn read_head(
        &self,
        stream: &mut impl Stream,
        method: Method,
        buf: &mut Vec<u8>,
    ) -> Result<Head, HttpError> {
        let mut chunk = [0u8; 4096];
        let mut interim = 0;
        loop {
            if let Some((head, used)) = head::parse(buf, method)? {
                buf.drain(..used);
                match head.status {
                    101 => return Err(HttpError::Head("a protocol switch")),
                    100..=199 if interim < MAX_INTERIM => {
                        interim += 1;
                        continue;
                    }
                    100..=199 => return Err(HttpError::Head("too many interim responses")),
                    _ => return Ok(head),
                }
            }
            if buf.len() >= self.limits.max_head {
                return Err(HttpError::TooLarge);
            }
            self.in_time()?;
            let room = (self.limits.max_head - buf.len()).min(chunk.len());
            let n = stream.recv(&mut chunk[..room])?;
            if n == 0 {
                return Err(HttpError::Truncated);
            }
            buf.extend_from_slice(&chunk[..n]);
        }
    }

    fn read_body(
        &self,
        stream: &mut impl Stream,
        framing: Framing,
        first: Vec<u8>,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>,
    ) -> Result<u64, HttpError> {
        let max = self.limits.max_body;
        let mut received = 0u64;
        let mut pass = |bytes: &[u8], received: &mut u64| -> Result<(), HttpError> {
            *received += bytes.len() as u64;
            if *received > max {
                return Err(HttpError::TooLarge);
            }
            sink(bytes)
        };
        let mut chunk = alloc::vec![0u8; 16 * 1024];
        match framing {
            Framing::Empty => Ok(0),
            Framing::Length(length) => {
                if length > max {
                    return Err(HttpError::TooLarge);
                }
                let take = first
                    .len()
                    .min(usize::try_from(length).unwrap_or(usize::MAX));
                pass(&first[..take], &mut received)?;
                while received < length {
                    self.in_time()?;
                    let want = usize::try_from(length - received)
                        .unwrap_or(usize::MAX)
                        .min(chunk.len());
                    let n = stream.recv(&mut chunk[..want])?;
                    if n == 0 {
                        return Err(HttpError::Truncated);
                    }
                    pass(&chunk[..n], &mut received)?;
                }
                Ok(received)
            }
            Framing::Chunked => {
                let mut chunked = body::Chunked::new();
                let mut input = first;
                loop {
                    chunked.feed(&input, &mut |data| pass(data, &mut received))?;
                    if chunked.done() {
                        return Ok(received);
                    }
                    self.in_time()?;
                    let n = stream.recv(&mut chunk)?;
                    if n == 0 {
                        return Err(HttpError::Truncated);
                    }
                    input = chunk[..n].to_vec();
                }
            }
            Framing::Close => {
                pass(&first, &mut received)?;
                loop {
                    self.in_time()?;
                    match stream.recv(&mut chunk)? {
                        0 => return Ok(received),
                        n => pass(&chunk[..n], &mut received)?,
                    }
                }
            }
        }
    }
}

/// A 206's `Content-Range` must start where the request's range did, and end
/// no later than it.
fn check_range(asked: Option<Range>, got: Option<ContentRange>) -> Result<(), HttpError> {
    let (Some(asked), Some(got)) = (asked, got) else {
        return Err(HttpError::Head("a 206 that is not for the range asked"));
    };
    let ends_within = asked.last.is_none_or(|last| got.last <= last);
    match got.first == asked.first && ends_within {
        true => Ok(()),
        false => Err(HttpError::Head("a 206 that is not for the range asked")),
    }
}
