//! A request head out, a response head in (headers by httparse). Of a
//! response, only what P5's client acts on is kept; a head that is unclear
//! about its body (two lengths, a length with chunking, a coding other than
//! chunked) is refused rather than guessed at (RFC 9112 §6.3).

use super::HttpError;
use super::url::Url;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// The most header lines a response may have.
pub const MAX_HEADERS: usize = 64;
/// The most header lines a response to [`super::Fetch::exchange`] may have:
/// every one is kept, so a page's many `Set-Cookie` lines must fit.
pub const MAX_KEPT_HEADERS: usize = 128;

/// A response's content coding (RFC 9110 §8.4): only what
/// [`super::Fetch::exchange`] asks for and decodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coding {
    Identity,
    Gzip,
    Brotli,
}

/// A response head read whole for [`super::Fetch::exchange`]: what the client
/// acts on, every header line as it came, and the body's coding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Full {
    pub head: Head,
    /// Every header line, in order: its name as sent, and its value's bytes
    /// with the spaces around them trimmed. A value need not be UTF-8 (RFC
    /// 9110 §5.5 allows obs-text), but those this client reads must be.
    pub headers: Vec<(String, Vec<u8>)>,
    pub coding: Coding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    /// A short text body, sent once: [`post`] writes it, and
    /// `Fetch::post` makes it, never following a redirect or trying again.
    Post,
}

/// The type a POST's body is sent as.
pub const POST_TYPE: &str = "text/plain; charset=utf-8";

/// The bytes asked for: `first` to `last` inclusive, or to the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub first: u64,
    pub last: Option<u64>,
}

/// What the client sends: no cookies, no compression, one request a
/// connection. A [`Method::Post`] here has an empty body; [`post`] gives it one.
pub fn request(method: Method, url: &Url, range: Option<Range>) -> Vec<u8> {
    write(method, url, range, &[], false)
}

/// [`request`], asking the server to keep the connection open for the next
/// request when `keep` (a [`super::Keep`]'s GET or HEAD; never a POST).
pub fn request_kept(method: Method, url: &Url, range: Option<Range>, keep: bool) -> Vec<u8> {
    write(method, url, range, &[], keep && method != Method::Post)
}

/// A POST of `body` to `url`: its head, with the body's length and
/// [`POST_TYPE`], then the body.
pub fn post(url: &Url, body: &[u8]) -> Vec<u8> {
    write(Method::Post, url, None, body, false)
}

fn write(method: Method, url: &Url, range: Option<Range>, body: &[u8], keep: bool) -> Vec<u8> {
    let verb = match method {
        Method::Get => "GET",
        Method::Head => "HEAD",
        Method::Post => "POST",
    };
    let mut head = format!(
        "{verb} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: kai\r\nAccept-Encoding: identity\r\nConnection: {}\r\n",
        url.path,
        url.authority(),
        if keep { "keep-alive" } else { "close" }
    );
    if let Some(Range { first, last }) = range {
        match last {
            Some(last) => head.push_str(&format!("Range: bytes={first}-{last}\r\n")),
            None => head.push_str(&format!("Range: bytes={first}-\r\n")),
        }
    }
    if method == Method::Post {
        head.push_str(&format!(
            "Content-Type: {POST_TYPE}\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

/// How a response's body is framed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// Exactly this many bytes.
    Length(u64),
    /// Chunked transfer coding.
    Chunked,
    /// Until the server closes, with a close_notify.
    Close,
    /// No body: HEAD, 204, 304, or a 1xx.
    Empty,
}

/// `Content-Range: bytes <first>-<last>/<total or *>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentRange {
    pub first: u64,
    pub last: u64,
    pub total: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub status: u16,
    pub framing: Framing,
    pub location: Option<String>,
    pub content_range: Option<ContentRange>,
    /// `Retry-After` in seconds, as a 429 or 503 gives it.
    pub retry_after: Option<u32>,
    /// `Content-Type`, when it is at most [`MAX_CONTENT_TYPE`] printable
    /// ASCII bytes; otherwise none.
    pub content_type: Option<String>,
    /// The status line's reason phrase ("OK"), when it is at most
    /// [`MAX_REASON`] printable ASCII bytes; otherwise none.
    pub reason: Option<String>,
    /// Whether the server closes the connection after this response: it said
    /// `Connection: close`. Such a connection is never kept.
    pub close: bool,
}

/// The longest `Content-Type` kept.
pub const MAX_CONTENT_TYPE: usize = 128;
/// The longest reason phrase kept.
pub const MAX_REASON: usize = 64;

/// `text` as a reason phrase: printable ASCII and at most [`MAX_REASON`]
/// bytes, or none. `wire` checks an answer's reason by the same rule.
pub fn reason(text: &str) -> Option<String> {
    let printable = text.bytes().all(|b| b == b' ' || b.is_ascii_graphic());
    (!text.is_empty() && text.len() <= MAX_REASON && printable).then(|| String::from(text))
}

/// The head of a response whose status line has come whole but whose head
/// could not be read: its status and reason, and nothing else, with no body.
/// A POST's answer is taken from it: the server has answered, so it has the
/// request (`Fetch::post`).
///
/// As httparse reads a head, blank lines before it are skipped, and a line may
/// end with a bare LF.
pub fn status_line(bytes: &[u8]) -> Option<Head> {
    let start = bytes.iter().position(|&b| b != b'\r' && b != b'\n')?;
    let bytes = &bytes[start..];
    let end = bytes.iter().position(|&b| b == b'\n')?;
    let line = &bytes[..end];
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let line = core::str::from_utf8(line).ok()?;
    let rest = line
        .strip_prefix("HTTP/1.1 ")
        .or_else(|| line.strip_prefix("HTTP/1.0 "))?;
    let (code, phrase) = match rest.split_once(' ') {
        Some((code, phrase)) => (code, phrase),
        None => (rest, ""),
    };
    let status = (code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit()))
        .then(|| code.parse::<u16>().ok())
        .flatten()
        .filter(|n| (100..=599).contains(n))?;
    Some(Head {
        status,
        framing: Framing::Empty,
        location: None,
        content_range: None,
        retry_after: None,
        content_type: None,
        reason: reason(phrase),
        close: true,
    })
}

/// The response head at the start of `bytes`, and its length; `Ok(None)`
/// if it has not all come yet.
pub fn parse(bytes: &[u8], method: Method) -> Result<Option<(Head, usize)>, HttpError> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    Ok(read(bytes, method, &mut headers, None)?.map(|(full, used)| (full.head, used)))
}

/// [`parse`] for [`super::Fetch::exchange`]: every header is kept, up to
/// [`MAX_KEPT_HEADERS`], and a value that is not UTF-8 is refused only where
/// this client reads it. With `decode`, a body coded `gzip` (or `x-gzip`) or
/// `br` is taken, one coding at most; any other coding is still refused.
pub fn parse_full(
    bytes: &[u8],
    method: Method,
    decode: bool,
) -> Result<Option<(Full, usize)>, HttpError> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_KEPT_HEADERS];
    read(bytes, method, &mut headers, Some(decode))
}

/// The names whose values this client reads, so must be text.
const READ: [&str; 8] = [
    "content-length",
    "transfer-encoding",
    "content-encoding",
    "location",
    "content-range",
    "retry-after",
    "connection",
    "content-type",
];

/// The head at the start of `bytes`. `full` is `None` for [`parse`]: no
/// header kept, any header not text refused, any coding refused. `Some(decode)`
/// keeps them all, and with `decode` takes gzip or br.
fn read<'b>(
    bytes: &'b [u8],
    method: Method,
    headers: &mut [httparse::Header<'b>],
    full: Option<bool>,
) -> Result<Option<(Full, usize)>, HttpError> {
    let mut response = httparse::Response::new(headers);
    let used = match response.parse(bytes) {
        Ok(httparse::Status::Complete(used)) => used,
        Ok(httparse::Status::Partial) => return Ok(None),
        Err(httparse::Error::TooManyHeaders) => return Err(HttpError::Head("too many headers")),
        Err(_) => return Err(HttpError::Head("not an HTTP/1.1 response head")),
    };
    if response.version != Some(1) {
        return Err(HttpError::Head("not HTTP/1.1"));
    }
    let status = response.code.ok_or(HttpError::Head("no status"))?;
    let mut length: Option<u64> = None;
    let mut chunked = false;
    let mut location = None;
    let mut content_range = None;
    let mut retry_after = None;
    let mut content_type = None;
    let mut close = false;
    let mut coding = Coding::Identity;
    let mut codings = 0;
    let mut kept = Vec::new();
    for header in response.headers.iter() {
        let name = header.name;
        if full.is_some() {
            kept.push((String::from(name), trim(header.value).to_vec()));
        }
        let value = match core::str::from_utf8(header.value) {
            Ok(value) => value.trim_matches([' ', '\t']),
            Err(_) if full.is_some() && !READ.iter().any(|r| name.eq_ignore_ascii_case(r)) => {
                continue;
            }
            Err(_) => return Err(HttpError::Head("a header is not text")),
        };
        if name.eq_ignore_ascii_case("content-length") {
            let n = number(value).ok_or(HttpError::Head("a bad Content-Length"))?;
            if length.is_some_and(|l| l != n) {
                return Err(HttpError::Head("two Content-Lengths"));
            }
            length = Some(n);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            if !value.eq_ignore_ascii_case("chunked") || chunked {
                return Err(HttpError::Head("a transfer coding other than chunked"));
            }
            chunked = true;
        } else if name.eq_ignore_ascii_case("content-encoding") && full.is_none() {
            // `parse`'s rule, unchanged: identity alone.
            if !value.eq_ignore_ascii_case("identity") {
                return Err(HttpError::Head("a compressed body"));
            }
        } else if name.eq_ignore_ascii_case("content-encoding") {
            let decode = full == Some(true);
            for token in value.split(',').map(|t| t.trim_matches([' ', '\t'])) {
                let this = match token {
                    t if t.eq_ignore_ascii_case("identity") => continue,
                    t if decode
                        && (t.eq_ignore_ascii_case("gzip") || t.eq_ignore_ascii_case("x-gzip")) =>
                    {
                        Coding::Gzip
                    }
                    t if decode && t.eq_ignore_ascii_case("br") => Coding::Brotli,
                    _ => return Err(HttpError::Head("a compressed body")),
                };
                codings += 1;
                if codings > 1 {
                    return Err(HttpError::Head("more than one content coding"));
                }
                coding = this;
            }
        } else if name.eq_ignore_ascii_case("location") {
            if location.is_some() {
                return Err(HttpError::Head("two Locations"));
            }
            location = Some(String::from(value));
        } else if name.eq_ignore_ascii_case("content-range") {
            content_range = Some(parse_content_range(value)?);
        } else if name.eq_ignore_ascii_case("retry-after") {
            retry_after = number(value).and_then(|n| u32::try_from(n).ok());
        } else if name.eq_ignore_ascii_case("connection") {
            close |= value.split(',').any(|token| {
                token
                    .trim_matches([' ', '\t'])
                    .eq_ignore_ascii_case("close")
            });
        } else if name.eq_ignore_ascii_case("content-type") {
            let printable = value.bytes().all(|b| b == b' ' || b.is_ascii_graphic());
            content_type =
                (value.len() <= MAX_CONTENT_TYPE && printable).then(|| String::from(value));
        }
    }
    if chunked && length.is_some() {
        return Err(HttpError::Head("both a length and chunking"));
    }
    let bodiless = method == Method::Head || status == 204 || status == 304 || status < 200;
    let framing = match (bodiless, chunked, length) {
        (true, _, _) => Framing::Empty,
        (false, true, _) => Framing::Chunked,
        (false, false, Some(n)) => Framing::Length(n),
        (false, false, None) => Framing::Close,
    };
    let head = Head {
        status,
        framing,
        location,
        content_range,
        retry_after,
        content_type,
        reason: response.reason.and_then(reason),
        close,
    };
    let full = Full {
        head,
        headers: kept,
        coding,
    };
    Ok(Some((full, used)))
}

/// `value` without the spaces and tabs around it.
fn trim(value: &[u8]) -> &[u8] {
    let blank = |b: &u8| *b == b' ' || *b == b'\t';
    let start = value.iter().position(|b| !blank(b)).unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|b| !blank(b))
        .map_or(start, |at| at + 1);
    &value[start..end]
}

/// A decimal number of at most 19 digits.
fn number(text: &str) -> Option<u64> {
    let good = !text.is_empty() && text.len() <= 19 && text.bytes().all(|b| b.is_ascii_digit());
    good.then(|| text.parse().ok()).flatten()
}

fn parse_content_range(value: &str) -> Result<ContentRange, HttpError> {
    let bad = HttpError::Head("a bad Content-Range");
    let spec = value.strip_prefix("bytes ").ok_or(bad.clone())?;
    let (range, total) = spec.split_once('/').ok_or(bad.clone())?;
    let (first, last) = range.split_once('-').ok_or(bad.clone())?;
    let (first, last) = (
        number(first).ok_or(bad.clone())?,
        number(last).ok_or(bad.clone())?,
    );
    let total = match total {
        "*" => None,
        total => Some(number(total).ok_or(bad.clone())?),
    };
    if last < first || total.is_some_and(|t| last >= t) {
        return Err(bad);
    }
    Ok(ContentRange { first, last, total })
}
