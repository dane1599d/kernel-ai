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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
}

/// The bytes asked for: `first` to `last` inclusive, or to the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub first: u64,
    pub last: Option<u64>,
}

/// What the client sends: no cookies, no compression, one request a
/// connection.
pub fn request(method: Method, url: &Url, range: Option<Range>) -> Vec<u8> {
    let verb = match method {
        Method::Get => "GET",
        Method::Head => "HEAD",
    };
    let mut head = format!(
        "{verb} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: kai\r\nAccept-Encoding: identity\r\nConnection: close\r\n",
        url.path,
        url.authority()
    );
    if let Some(Range { first, last }) = range {
        match last {
            Some(last) => head.push_str(&format!("Range: bytes={first}-{last}\r\n")),
            None => head.push_str(&format!("Range: bytes={first}-\r\n")),
        }
    }
    head.push_str("\r\n");
    head.into_bytes()
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
}

/// The longest `Content-Type` kept.
pub const MAX_CONTENT_TYPE: usize = 128;

/// The response head at the start of `bytes`, and its length; `Ok(None)`
/// if it has not all come yet.
pub fn parse(bytes: &[u8], method: Method) -> Result<Option<(Head, usize)>, HttpError> {
    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut response = httparse::Response::new(&mut headers);
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
    for header in response.headers.iter() {
        let value = core::str::from_utf8(header.value)
            .map_err(|_| HttpError::Head("a header is not text"))?
            .trim_matches([' ', '\t']);
        let name = header.name;
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
        } else if name.eq_ignore_ascii_case("content-encoding") {
            if !value.eq_ignore_ascii_case("identity") {
                return Err(HttpError::Head("a compressed body"));
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
    };
    Ok(Some((head, used)))
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
