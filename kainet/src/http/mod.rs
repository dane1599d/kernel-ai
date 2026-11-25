//! HTTP/1.1 for P5 (plan p5-networking, task 5): GET and HEAD, a byte range,
//! and a POST of a short text body made once ([`Fetch::post`]),
//! a body by length, by chunks or until a close_notify, at most five
//! redirects, and limits on size and time. No cookies and no compression;
//! one request a connection, but for a [`Keep`]'s GETs and HEADs to one host,
//! which reuse it while each response is read whole. Every redirect's host is checked with the
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
    /// A POST failed after a byte of it may have gone out and before a whole
    /// status line came: the server may have it, so it is not tried again
    /// ([`Fetch::post`]).
    Cut,
}

impl HttpError {
    /// The error in fixed words a person may read: a TLS failure as its kind
    /// ([`crate::fetch::tls_kind`]), never rustls's own text, which can carry
    /// the names on a certificate from a private network (audit K #17).
    pub fn plain(&self) -> String {
        match self {
            HttpError::Url(why)
            | HttpError::Connect(why)
            | HttpError::Head(why)
            | HttpError::Body(why) => String::from(*why),
            HttpError::Tls(tls) => String::from(crate::fetch::tls_kind(tls)),
            HttpError::Truncated => String::from("the answer was cut short"),
            HttpError::TooLarge => String::from("the answer was too large"),
            HttpError::TooManyRedirects => String::from("more than five redirects"),
            HttpError::Refused(host) => alloc::format!("{host} is not a host this may reach"),
            HttpError::Timeout => String::from("it timed out"),
            HttpError::RangeIgnored => String::from("the server ignored the range"),
            HttpError::Cut => String::from("the connection was cut; the server may have it"),
        }
    }
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
    /// A new connection, never one kept: for a POST, and for a GET tried
    /// again after a kept connection failed it.
    fn fresh(&mut self, url: &Url) -> Result<Self::Stream, HttpError> {
        self.connect(url)
    }
    /// Whether requests ask the server to keep the connection ([`Keep`]).
    fn keeps(&self) -> bool {
        false
    }
    /// Whether the last [`Connect::connect`] gave a kept connection.
    fn reused(&self) -> bool {
        false
    }
    /// Takes back `stream`, whose response to a request for `url` was read
    /// whole, to carry the next request to the same host and port.
    fn give_back(&mut self, url: &Url, stream: Self::Stream) {
        let _ = (url, stream);
    }
}

/// The most connections a [`Keep`] holds: a download's hosts (a release
/// host and its assets host, a resolve host and its storage host).
pub const MAX_KEPT: usize = 4;

/// The longest redirect body read through so its connection can be kept.
const MAX_DRAINED: u64 = 4096;

/// One caller's connections, kept between its GETs and HEADs (a download's
/// chunks): one a host and port, at most [`MAX_KEPT`], each reused only for
/// its own host and port through the same `inner`, which dials one
/// `NetDest`. A connection is kept only when its response was read whole and
/// the server did not say it closes; any error or cut drops it, and a POST
/// never takes one ([`Connect::fresh`]). A `Keep` is its caller's alone:
/// never shared.
pub struct Keep<C: Connect> {
    inner: C,
    kept: Vec<(String, u16, C::Stream)>,
    reused: bool,
}

impl<C: Connect> Keep<C> {
    pub fn new(inner: C) -> Self {
        Self {
            inner,
            kept: Vec::new(),
            reused: false,
        }
    }

    /// How many connections are kept now.
    pub fn held(&self) -> usize {
        self.kept.len()
    }

    /// Whether a connection is kept now.
    pub fn holds(&self) -> bool {
        !self.kept.is_empty()
    }

    /// Drops every kept connection.
    pub fn drop_all(&mut self) {
        self.kept.clear();
    }

    /// The kept connections, to set each one's next deadline.
    pub fn kept_mut(&mut self) -> impl Iterator<Item = &mut C::Stream> {
        self.kept.iter_mut().map(|(_, _, stream)| stream)
    }

    fn take(&mut self, url: &Url) -> Option<C::Stream> {
        let at = self
            .kept
            .iter()
            .position(|(host, port, _)| *host == url.host && *port == url.port)?;
        Some(self.kept.remove(at).2)
    }

    pub fn inner_mut(&mut self) -> &mut C {
        &mut self.inner
    }
}

impl<C: Connect> Connect for Keep<C> {
    type Stream = C::Stream;

    fn connect(&mut self, url: &Url) -> Result<Self::Stream, HttpError> {
        match self.take(url) {
            Some(stream) => {
                self.reused = true;
                Ok(stream)
            }
            None => self.fresh(url),
        }
    }

    fn fresh(&mut self, url: &Url) -> Result<Self::Stream, HttpError> {
        drop(self.take(url));
        self.reused = false;
        match self.inner.connect(url) {
            // The kept connections may be what the transport is short of (the
            // OS gives a process two bulk streams): they go, and the
            // connection is tried once more.
            Err(HttpError::Connect(_)) if !self.kept.is_empty() => {
                self.kept.clear();
                self.inner.connect(url)
            }
            other => other,
        }
    }

    fn keeps(&self) -> bool {
        true
    }

    fn reused(&self) -> bool {
        self.reused
    }

    fn give_back(&mut self, url: &Url, stream: Self::Stream) {
        drop(self.take(url));
        if self.kept.len() == MAX_KEPT {
            self.kept.remove(0);
        }
        self.kept.push((url.host.clone(), url.port, stream));
    }
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

    /// A POST of `body` to `url`, made once: the server gets it at most once.
    /// - It is never followed to another URL. A 3xx, 303 included, is the
    ///   answer, with [`Fetched::unfollowed`] saying where it led.
    /// - It is never tried again, on this connection or another.
    /// - Its answer is its head alone: no body is read, whatever the status.
    /// - Once a whole final status line (200 or more) has come, the server
    ///   has answered, so the answer is that status: a head that does not
    ///   read, or a connection that ends inside the head, keeps the status and
    ///   drops the rest ([`head::status_line`]). An interim 1xx is no answer:
    ///   it is skipped, and a cut after it is a cut.
    /// - Before any byte is written, a failure says it was not sent: a refused
    ///   host, the deadline (checked again once the connection is open), no
    ///   connection or no TLS. Between the first byte written and a whole
    ///   final status line, any failure is [`HttpError::Cut`]: the server may
    ///   have it.
    pub fn post(
        &self,
        connect: &mut impl Connect,
        url: &Url,
        body: &[u8],
    ) -> Result<Fetched, HttpError> {
        if !(self.allow)(&url.host, url.port) {
            return Err(HttpError::Refused(url.host.clone()));
        }
        self.in_time()?;
        let mut stream = connect.fresh(url)?;
        self.in_time()?;
        stream
            .send(&head::post(url, body))
            .map_err(|_| HttpError::Cut)?;
        let head = self.post_head(&mut stream)?;
        let unfollowed = match redirects(head.status) {
            true => head.location.as_deref().and_then(|at| url.join(at).ok()),
            false => None,
        };
        Ok(Fetched {
            url: url.clone(),
            head,
            received: 0,
            redirects: 0,
            unfollowed,
        })
    }

    /// A POST's answer: its final head (200 or more). Interim 1xx heads are
    /// skipped, however many, within [`Limits::max_head`] bytes of heads in
    /// all; a 101 is no answer to a POST. Once a final status line has come
    /// whole, any failure gives that status; before it, any failure, a cut
    /// after a 1xx included, is [`HttpError::Cut`].
    fn post_head(&self, stream: &mut impl Stream) -> Result<Head, HttpError> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        // Bytes of heads read, interim ones included: the bound on them all.
        let mut read = 0;
        let answered = |buf: &[u8]| {
            head::status_line(buf)
                .filter(|head| head.status >= 200)
                .ok_or(HttpError::Cut)
        };
        loop {
            match head::parse(&buf, Method::Post) {
                Ok(Some((head, _))) if head.status >= 200 => return Ok(head),
                Ok(Some((head, _))) if head.status == 101 => return Err(HttpError::Cut),
                Ok(Some((_, used))) => {
                    buf.drain(..used);
                    continue;
                }
                Ok(None) => {}
                Err(_) => return answered(&buf),
            }
            if read >= self.limits.max_head || self.in_time().is_err() {
                return answered(&buf);
            }
            let room = (self.limits.max_head - read).min(chunk.len());
            match stream.recv(&mut chunk[..room]) {
                Ok(n) if n > 0 => {
                    read += n;
                    buf.extend_from_slice(&chunk[..n]);
                }
                _ => return answered(&buf),
            }
        }
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
        // A POST is made once, never walked: a redirect would send it again.
        if method == Method::Post {
            return Err(HttpError::Url("a POST is made by Fetch::post"));
        }
        let mut url = url.clone();
        if !(self.allow)(&url.host, url.port) {
            return Err(HttpError::Refused(url.host));
        }
        for followed in 0..=self.limits.max_redirects {
            self.in_time()?;
            let (mut stream, head, buf) = self.ask(connect, method, &url, range)?;
            if redirects(head.status) {
                let location = head
                    .location
                    .as_deref()
                    .ok_or(HttpError::Head("a redirect with no Location"))?;
                let next = url.join(location).map_err(HttpError::Url)?;
                self.keep_redirect(connect, &url, &head, stream, buf);
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
                self.keep_redirect(connect, &url, &head, stream, buf);
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
            // Read whole and not closing: the connection may carry the next.
            if connect.keeps() && !head.close && head.framing != Framing::Close {
                connect.give_back(&url, stream);
            }
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

    /// The connection of an answer whose body is not passed on (a redirect, a
    /// 404), kept when that short body is read through and dropped: the next
    /// request to `url`'s host takes it. Anything else drops it.
    fn keep_redirect<C: Connect>(
        &self,
        connect: &mut C,
        url: &Url,
        head: &Head,
        mut stream: C::Stream,
        buf: Vec<u8>,
    ) {
        let short = match head.framing {
            Framing::Empty => true,
            Framing::Length(n) => n <= MAX_DRAINED,
            Framing::Chunked | Framing::Close => false,
        };
        if !connect.keeps() || head.close || !short {
            return;
        }
        let drained = Self {
            limits: Limits {
                max_body: MAX_DRAINED,
                ..self.limits
            },
            allow: self.allow,
            clock: self.clock,
        }
        .read_body(&mut stream, head.framing, buf, &mut |_| Ok(()));
        if drained.is_ok() {
            connect.give_back(url, stream);
        }
    }

    /// Sends the request for `url` and reads its head: the stream, the head
    /// and any body bytes that came with it. A kept connection the server
    /// has closed meanwhile fails before a head comes; the request is then
    /// made once more on a new connection, nothing having been passed on.
    fn ask<C: Connect>(
        &self,
        connect: &mut C,
        method: Method,
        url: &Url,
        range: Option<Range>,
    ) -> Result<(C::Stream, Head, Vec<u8>), HttpError> {
        let request = head::request_kept(method, url, range, connect.keeps());
        let mut stream = connect.connect(url)?;
        let reused = connect.reused();
        let mut buf = Vec::new();
        let first = match stream.send(&request) {
            Ok(()) => self.read_head(&mut stream, method, &mut buf),
            Err(error) => Err(error),
        };
        match first {
            Ok(head) => Ok((stream, head, buf)),
            Err(HttpError::Timeout) => Err(HttpError::Timeout),
            Err(_) if reused => {
                drop(stream);
                self.in_time()?;
                let mut stream = connect.fresh(url)?;
                stream.send(&request)?;
                let mut buf = Vec::new();
                let head = self.read_head(&mut stream, method, &mut buf)?;
                Ok((stream, head, buf))
            }
            Err(error) => Err(error),
        }
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
