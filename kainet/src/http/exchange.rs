//! One HTTP/1.1 exchange for the browser's `webnet` (plan P6 F-4, the host
//! half): any of the request methods, the caller's own request headers, a
//! request body of any size sent as it is read, every response header kept,
//! the response body of any status streamed to the caller and, when asked,
//! its gzip or brotli coding undone. It is [`Fetch::exchange`].
//!
//! - **One exchange, no redirect followed.** A 3xx comes back as itself, with
//!   [`Answer::location`] made absolute: the browser walks redirects itself,
//!   since each hop has its own cookies and checks.
//! - **A request that is not safe** (RFC 9110 §9.2.1: anything but GET, HEAD
//!   and OPTIONS), or that carries a body, never takes a kept connection and
//!   is never tried again; once a byte of it may have gone out, any failure
//!   before the final head is [`HttpError::Cut`]: the server may have it.
//! - **A safe request** may take a kept connection, and is made once more on
//!   a new one if that kept connection fails before any head comes.
//! - **Kept connections:** any exchange whose response was read whole, from a
//!   server that did not say it closes, gives its connection back to a
//!   [`super::Keep`].
//! - **Limits:** [`Limits::max_head`] bounds the response head;
//!   [`Limits::max_body`] bounds both the body's bytes on the wire and its
//!   decoded bytes; the deadline is checked between every read and write.
//!
//! `kai:net@0.1.0`'s fetch (`crate::fetch`) and [`Fetch::run`] are unchanged.

use super::coding::Decoder;
use super::head::{self, Coding, Full, Head, Method};
use super::{Connect, Fetch, Framing, HttpError, Stream, Url};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// A request method (RFC 9110 §9.3, without CONNECT and TRACE).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
    Options,
}

impl Verb {
    pub fn name(self) -> &'static str {
        match self {
            Verb::Get => "GET",
            Verb::Head => "HEAD",
            Verb::Post => "POST",
            Verb::Put => "PUT",
            Verb::Patch => "PATCH",
            Verb::Delete => "DELETE",
            Verb::Options => "OPTIONS",
        }
    }

    /// Whether it is safe (RFC 9110 §9.2.1), so may be made again.
    pub fn safe(self) -> bool {
        matches!(self, Verb::Get | Verb::Head | Verb::Options)
    }

    /// Whether a body is expected with it, so an empty one is sent as
    /// `Content-Length: 0` (RFC 9110 §8.6).
    fn takes_body(self) -> bool {
        matches!(self, Verb::Post | Verb::Put | Verb::Patch)
    }
}

/// A request's body.
pub enum Body<'a> {
    Empty,
    /// These bytes, of any size, sent with their `Content-Length`.
    Bytes(&'a [u8]),
    /// Bytes pulled from `read` as they are sent, each call filling what it
    /// can of the buffer it is given and giving 0 at its end. With `length`
    /// the body is exactly that many bytes (`Content-Length`), and a `read`
    /// that ends early or gives more is refused; without, it is sent in
    /// chunks (`Transfer-Encoding: chunked`) until `read` gives 0.
    Stream {
        length: Option<u64>,
        read: &'a mut dyn FnMut(&mut [u8]) -> Result<usize, HttpError>,
    },
}

impl Body<'_> {
    fn is_empty(&self) -> bool {
        match self {
            Body::Empty => true,
            Body::Bytes(bytes) => bytes.is_empty(),
            Body::Stream { length, .. } => *length == Some(0),
        }
    }
}

/// One request.
pub struct Request<'a> {
    pub verb: Verb,
    pub url: &'a Url,
    /// The caller's headers, sent in order after the client's own. Each is
    /// checked ([`check_header`]); the names the client sets itself are
    /// refused ([`OWN_HEADERS`]), but `User-Agent` replaces the client's.
    pub headers: &'a [(&'a str, &'a str)],
    pub body: Body<'a>,
    /// Ask for gzip and brotli (`Accept-Encoding: gzip, br`) and undo them;
    /// otherwise only `identity` is asked for and a coded body is refused.
    pub decode: bool,
}

/// What an exchange came to: its head, every header, and how much body.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub head: Head,
    /// Every response header line, as [`head::Full::headers`] keeps them.
    pub headers: Vec<(String, Vec<u8>)>,
    /// The coding the body came in; what reached the sink is decoded.
    pub coding: Coding,
    /// Body bytes passed to the sink, decoded.
    pub received: u64,
    /// Body bytes read from the connection, before decoding.
    pub wire: u64,
    /// Where a 3xx's `Location` leads, made absolute, when it is a URL this
    /// client can fetch.
    pub location: Option<Url>,
    /// Whether the exchange went over a kept connection.
    pub reused: bool,
}

/// The most headers a caller may add.
pub const MAX_REQUEST_HEADERS: usize = 64;
/// The longest request head, the caller's headers and the request line in
/// it.
pub const MAX_REQUEST_HEAD: usize = 32 * 1024;
/// The longest header name a caller may give.
pub const MAX_NAME: usize = 256;
/// The longest header value a caller may give.
pub const MAX_VALUE: usize = 8 * 1024;
/// Bytes of a streamed request body sent at a time.
const PIECE: usize = 16 * 1024;

/// The headers the client sets itself, which a caller may not: the framing,
/// the connection, the host, the codings asked for. No `Expect`: the body
/// goes with the head, never waiting for a 100.
pub const OWN_HEADERS: [&str; 12] = [
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "upgrade",
    "expect",
    "accept-encoding",
    "http2-settings",
];

/// Whether `name: value` may be sent: a token name (RFC 9110 §5.6.2) of at
/// most [`MAX_NAME`] bytes that is not one of [`OWN_HEADERS`], and a value of
/// at most [`MAX_VALUE`] visible ASCII bytes, spaces and tabs, with none at
/// either end. No byte of it can end a line, so a caller cannot add a header
/// or a request of its own.
pub fn check_header(name: &str, value: &str) -> Result<(), HttpError> {
    let tchar = |b: u8| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b);
    if name.is_empty() || name.len() > MAX_NAME || !name.bytes().all(tchar) {
        return Err(HttpError::Url("a request header's name is not a token"));
    }
    if OWN_HEADERS.iter().any(|own| name.eq_ignore_ascii_case(own)) {
        return Err(HttpError::Url("a request header the client sets itself"));
    }
    let text = |b: u8| b == b'\t' || (b' '..=b'~').contains(&b);
    let blank = |b: Option<u8>| matches!(b, Some(b' ' | b'\t'));
    let ends = blank(value.bytes().next()) || blank(value.bytes().next_back());
    if value.len() > MAX_VALUE || !value.bytes().all(text) || ends {
        return Err(HttpError::Url(
            "a request header's value is not one line of text",
        ));
    }
    Ok(())
}

/// `request`'s head: the request line, the client's headers, the caller's,
/// and the body's framing.
pub fn request_head(request: &Request<'_>, keep: bool) -> Result<Vec<u8>, HttpError> {
    if request.headers.len() > MAX_REQUEST_HEADERS {
        return Err(HttpError::Url("too many request headers"));
    }
    for (name, value) in request.headers {
        check_header(name, value)?;
    }
    if matches!(request.verb, Verb::Get | Verb::Head) && !request.body.is_empty() {
        return Err(HttpError::Url("a GET or HEAD carries no body"));
    }
    let url = request.url;
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\n",
        request.verb.name(),
        url.path,
        url.authority()
    );
    let theirs = |name: &str| {
        request
            .headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(name))
    };
    if !theirs("user-agent") {
        head.push_str("User-Agent: kai\r\n");
    }
    head.push_str(match request.decode {
        true => "Accept-Encoding: gzip, br\r\n",
        false => "Accept-Encoding: identity\r\n",
    });
    head.push_str(match keep {
        true => "Connection: keep-alive\r\n",
        false => "Connection: close\r\n",
    });
    for (name, value) in request.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    match &request.body {
        Body::Empty if request.verb.takes_body() => head.push_str("Content-Length: 0\r\n"),
        Body::Empty => {}
        Body::Bytes(bytes) => head.push_str(&format!("Content-Length: {}\r\n", bytes.len())),
        Body::Stream {
            length: Some(n), ..
        } => head.push_str(&format!("Content-Length: {n}\r\n")),
        Body::Stream { length: None, .. } => head.push_str("Transfer-Encoding: chunked\r\n"),
    }
    head.push_str("\r\n");
    if head.len() > MAX_REQUEST_HEAD {
        return Err(HttpError::Url("the request head is too long"));
    }
    Ok(head.into_bytes())
}

impl Fetch<'_> {
    /// One exchange: `request` sent, its answer's head read, and its body,
    /// whatever its status, passed to `sink` as it arrives, decoded. See the
    /// module's doc for what is kept, tried again and refused.
    pub fn exchange(
        &self,
        connect: &mut impl Connect,
        mut request: Request<'_>,
        sink: &mut dyn FnMut(&[u8]) -> Result<(), HttpError>,
    ) -> Result<Answer, HttpError> {
        let url = request.url;
        if !(self.allow)(&url.host, url.port) {
            return Err(HttpError::Refused(url.host.clone()));
        }
        let again = request.verb.safe() && request.body.is_empty();
        let head = request_head(&request, connect.keeps())?;
        self.in_time()?;
        let mut stream = match again {
            true => connect.connect(url)?,
            false => connect.fresh(url)?,
        };
        let reused = connect.reused();
        self.in_time()?;
        let method = match request.verb {
            Verb::Head => Method::Head,
            _ => Method::Get,
        };
        let first = self
            .send(&mut stream, &head, &mut request.body)
            .and_then(|()| self.read_full(&mut stream, method, request.decode));
        let (mut stream, full, buf, reused) = match first {
            Ok((full, buf)) => (stream, full, buf, reused),
            Err(_) if !again => return Err(HttpError::Cut),
            Err(HttpError::Timeout) => return Err(HttpError::Timeout),
            Err(_) if reused => {
                drop(stream);
                self.in_time()?;
                let mut stream = connect.fresh(url)?;
                stream.send(&head)?;
                let (full, buf) = self.read_full(&mut stream, method, request.decode)?;
                (stream, full, buf, false)
            }
            Err(error) => return Err(error),
        };
        let Full {
            head,
            headers,
            coding,
        } = full;
        let mut decoder = Decoder::new(coding, self.limits.max_body);
        let wire = self.read_body(&mut stream, head.framing, buf, &mut |bytes| {
            decoder.feed(bytes, sink)
        })?;
        decoder.finish(wire)?;
        if connect.keeps() && !head.close && head.framing != Framing::Close {
            connect.give_back(url, stream);
        }
        let location = match super::redirects(head.status) {
            true => head.location.as_deref().and_then(|at| url.join(at).ok()),
            false => None,
        };
        Ok(Answer {
            head,
            headers,
            coding,
            received: decoder.decoded(),
            wire,
            location,
            reused,
        })
    }

    /// Sends `head`, then `body`, checking the deadline between pieces.
    fn send(
        &self,
        stream: &mut impl Stream,
        head: &[u8],
        body: &mut Body<'_>,
    ) -> Result<(), HttpError> {
        match body {
            Body::Empty => stream.send(head),
            // A short body goes with its head, in one write.
            Body::Bytes(bytes) if bytes.len() <= PIECE => {
                let mut whole = head.to_vec();
                whole.extend_from_slice(bytes);
                stream.send(&whole)
            }
            Body::Bytes(bytes) => {
                stream.send(head)?;
                for piece in bytes.chunks(PIECE) {
                    self.in_time()?;
                    stream.send(piece)?;
                }
                Ok(())
            }
            Body::Stream { length, read } => {
                stream.send(head)?;
                let mut buf = vec![0u8; PIECE];
                let mut sent = 0u64;
                loop {
                    self.in_time()?;
                    let room = match length {
                        // One byte past the length, to see a read that gives more.
                        Some(n) => {
                            usize::try_from(*n - sent).map_or(PIECE, |left| left.min(PIECE - 1) + 1)
                        }
                        None => PIECE,
                    };
                    let got = read(&mut buf[..room])?;
                    if got > room {
                        return Err(HttpError::Body("a request body's read overran"));
                    }
                    sent += got as u64;
                    match length {
                        Some(n) if sent > *n => {
                            return Err(HttpError::Body("a request body runs past its length"));
                        }
                        Some(n) if got == 0 && sent < *n => {
                            return Err(HttpError::Body("a request body ends short of its length"));
                        }
                        Some(_) if got == 0 => return Ok(()),
                        Some(_) => stream.send(&buf[..got])?,
                        None if got == 0 => return stream.send(b"0\r\n\r\n"),
                        None => {
                            let mut piece = format!("{got:x}\r\n").into_bytes();
                            piece.extend_from_slice(&buf[..got]);
                            piece.extend_from_slice(b"\r\n");
                            stream.send(&piece)?;
                        }
                    }
                }
            }
        }
    }

    /// [`Fetch::read_head`] for an exchange: the final head whole, past any
    /// 1xx interim ones, with any body bytes that came with it.
    fn read_full(
        &self,
        stream: &mut impl Stream,
        method: Method,
        decode: bool,
    ) -> Result<(Full, Vec<u8>), HttpError> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let mut interim = 0;
        loop {
            if let Some((full, used)) = head::parse_full(&buf, method, decode)? {
                buf.drain(..used);
                match full.head.status {
                    101 => return Err(HttpError::Head("a protocol switch")),
                    100..=199 if interim < super::MAX_INTERIM => {
                        interim += 1;
                        continue;
                    }
                    100..=199 => return Err(HttpError::Head("too many interim responses")),
                    _ => return Ok((full, buf)),
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
}
