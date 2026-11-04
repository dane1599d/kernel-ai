//! The mock's side of HTTP/1.1: a request head in (GET or HEAD, `Host:` and
//! at most one byte range), a response head out. Everything else a client
//! sends is refused with a status, never guessed at.

/// The longest request target taken.
const MAX_TARGET: usize = 2048;
/// The most header lines taken.
const MAX_HEADERS: usize = 64;

/// A byte range a request asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    /// `bytes=<first>-` or `bytes=<first>-<last>`, inclusive.
    From { first: usize, last: Option<usize> },
    /// `bytes=-<n>`: the last `n` bytes.
    Suffix(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    /// The `Host:` header's name, lower-cased, without a port.
    pub host: String,
    pub range: Option<Range>,
}

/// The request in `head`, which ends with its blank line, or the status that
/// refuses it.
pub fn parse(head: &[u8]) -> Result<Request, u16> {
    let text = std::str::from_utf8(head).map_err(|_| 400u16)?;
    let text = text.strip_suffix("\r\n\r\n").ok_or(400u16)?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().ok_or(400u16)?;
    let mut words = request_line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (words.next(), words.next(), words.next(), words.next())
    else {
        return Err(400);
    };
    if version != "HTTP/1.1" {
        return Err(505);
    }
    if method != "GET" && method != "HEAD" {
        return Err(405);
    }
    if !target.starts_with('/') || target.len() > MAX_TARGET || !target.bytes().all(is_target_byte)
    {
        return Err(400);
    }
    let mut host = None;
    let mut range = None;
    for (count, line) in lines.enumerate() {
        if count >= MAX_HEADERS {
            return Err(431);
        }
        // A folded line, or one with no name, is not taken (RFC 9112 §5.2).
        let (name, value) = line.split_once(':').ok_or(400u16)?;
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(400);
        }
        let value = value.trim_matches([' ', '\t']);
        if name.eq_ignore_ascii_case("host") {
            if host.is_some() {
                return Err(400);
            }
            host = Some(host_name(value)?);
        } else if name.eq_ignore_ascii_case("range") {
            if range.is_some() {
                return Err(400);
            }
            range = Some(parse_range(value)?);
        }
    }
    Ok(Request {
        method: method.into(),
        path: target.into(),
        host: host.ok_or(400u16)?,
        range,
    })
}

/// Bytes a request target may hold: visible ASCII, no space.
fn is_target_byte(byte: u8) -> bool {
    byte.is_ascii_graphic()
}

/// `value` as a host name: lower-cased, its `:443` dropped.
fn host_name(value: &str) -> Result<String, u16> {
    let name = value.strip_suffix(":443").unwrap_or(value);
    let good = !name.is_empty()
        && name.len() <= 253
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
    good.then(|| name.to_ascii_lowercase()).ok_or(400)
}

/// One `bytes=` range; a list of them is 416, as the mock serves one.
fn parse_range(value: &str) -> Result<Range, u16> {
    let spec = value.strip_prefix("bytes=").ok_or(400u16)?;
    if spec.contains(',') {
        return Err(416);
    }
    let (first, last) = spec.split_once('-').ok_or(400u16)?;
    let number = |s: &str| -> Result<usize, u16> {
        if s.is_empty() || s.len() > 19 || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(400);
        }
        s.parse().map_err(|_| 400)
    };
    match (first.is_empty(), last.is_empty()) {
        (true, false) => Ok(Range::Suffix(number(last)?)),
        (false, true) => Ok(Range::From {
            first: number(first)?,
            last: None,
        }),
        (false, false) => {
            let (first, last) = (number(first)?, number(last)?);
            if last < first {
                return Err(416);
            }
            Ok(Range::From {
                first,
                last: Some(last),
            })
        }
        (true, true) => Err(400),
    }
}

/// `range` over a file of `len` bytes, as `[start, end)`; `None` if none of
/// it is there (416).
pub fn resolve(range: Range, len: usize) -> Option<(usize, usize)> {
    match range {
        Range::From { first, last } => {
            if first >= len {
                return None;
            }
            let last = last.map_or(len - 1, |last| last.min(len - 1));
            Some((first, last + 1))
        }
        Range::Suffix(0) => None,
        Range::Suffix(n) => (len > 0).then(|| (len - n.min(len), len)),
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        302 => "Found",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        416 => "Range Not Satisfiable",
        421 => "Misdirected Request",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        505 => "HTTP Version Not Supported",
        _ => "Unknown",
    }
}

/// A response head: the status, `headers`, and a body of `length` bytes.
pub fn head(status: u16, headers: &[(&'static str, String)], length: usize) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status} {}\r\nContent-Length: {length}\r\nConnection: close\r\n",
        reason(status)
    );
    for (name, value) in headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("\r\n");
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(head: &str) -> Result<Request, u16> {
        parse(head.as_bytes())
    }

    #[test]
    fn a_get_with_its_host_and_a_range_is_taken() {
        let got = request(
            "GET /a/b.json HTTP/1.1\r\nHost: Updates.Kai.Test:443\r\nRange: bytes=2-5\r\n\r\n",
        );
        assert_eq!(
            got,
            Ok(Request {
                method: "GET".into(),
                path: "/a/b.json".into(),
                host: "updates.kai.test".into(),
                range: Some(Range::From {
                    first: 2,
                    last: Some(5)
                }),
            })
        );
        let head = request("HEAD / HTTP/1.1\r\nhost: x.test\r\n\r\n").unwrap();
        assert_eq!((head.method.as_str(), head.range), ("HEAD", None));
    }

    #[test]
    fn hostile_heads_are_refused_by_status() {
        for (head, status) in [
            ("GET / HTTP/1.1\r\n\r\n", 400),
            (
                "GET / HTTP/1.1\r\nHost: a.test\r\nHost: b.test\r\n\r\n",
                400,
            ),
            ("GET / HTTP/1.1\r\nHost: a test\r\n\r\n", 400),
            ("GET / HTTP/1.1\r\nHost: a.test:8080\r\n\r\n", 400),
            ("POST / HTTP/1.1\r\nHost: a.test\r\n\r\n", 405),
            ("GET / HTTP/1.0\r\nHost: a.test\r\n\r\n", 505),
            ("GET  / HTTP/1.1\r\nHost: a.test\r\n\r\n", 400),
            ("GET a HTTP/1.1\r\nHost: a.test\r\n\r\n", 400),
            ("GET /\x01 HTTP/1.1\r\nHost: a.test\r\n\r\n", 400),
            ("GET / HTTP/1.1 extra\r\nHost: a.test\r\n\r\n", 400),
            ("GET / HTTP/1.1\r\nHost: a.test\r\n folded\r\n\r\n", 400),
            ("GET / HTTP/1.1\r\nHost: a.test\r\nno colon\r\n\r\n", 400),
            ("GET / HTTP/1.1\r\nHost: a.test\r\n: empty\r\n\r\n", 400),
            ("GET / HTTP/1.1\r\nHost: a.test\r\n", 400),
            (
                "GET / HTTP/1.1\r\nHost: a.test\r\nRange: lines=1-2\r\n\r\n",
                400,
            ),
            (
                "GET / HTTP/1.1\r\nHost: a.test\r\nRange: bytes=1-2,4-5\r\n\r\n",
                416,
            ),
            (
                "GET / HTTP/1.1\r\nHost: a.test\r\nRange: bytes=5-2\r\n\r\n",
                416,
            ),
            (
                "GET / HTTP/1.1\r\nHost: a.test\r\nRange: bytes=-\r\n\r\n",
                400,
            ),
            (
                "GET / HTTP/1.1\r\nHost: a.test\r\nRange: bytes=+1-2\r\n\r\n",
                400,
            ),
            (
                "GET / HTTP/1.1\r\nHost: a.test\r\nRange: bytes=99999999999999999999-\r\n\r\n",
                400,
            ),
            (
                "GET / HTTP/1.1\r\nHost: a.test\r\nRange: bytes=1-2\r\nRange: bytes=3-4\r\n\r\n",
                400,
            ),
        ] {
            assert_eq!(request(head), Err(status), "{head:?}");
        }
        assert_eq!(parse(b"GET / HTTP/1.1\r\nHost: \xff\r\n\r\n"), Err(400));
        let long = format!(
            "GET /{} HTTP/1.1\r\nHost: a.test\r\n\r\n",
            "a".repeat(MAX_TARGET)
        );
        assert_eq!(request(&long), Err(400));
        let many = format!(
            "GET / HTTP/1.1\r\nHost: a.test\r\n{}\r\n",
            "X-A: b\r\n".repeat(MAX_HEADERS)
        );
        assert_eq!(request(&many), Err(431));
    }

    #[test]
    fn a_range_resolves_within_the_file_or_not_at_all() {
        let from = |first, last| Range::From { first, last };
        assert_eq!(resolve(from(0, None), 10), Some((0, 10)));
        assert_eq!(resolve(from(2, Some(5)), 10), Some((2, 6)));
        assert_eq!(resolve(from(8, Some(50)), 10), Some((8, 10)));
        assert_eq!(resolve(from(10, None), 10), None);
        assert_eq!(resolve(from(0, None), 0), None);
        assert_eq!(resolve(Range::Suffix(3), 10), Some((7, 10)));
        assert_eq!(resolve(Range::Suffix(30), 10), Some((0, 10)));
        assert_eq!(resolve(Range::Suffix(0), 10), None);
        assert_eq!(resolve(Range::Suffix(3), 0), None);
    }

    #[test]
    fn a_response_head_names_its_length_and_closes() {
        let head = head(206, &[("Content-Range", "bytes 2-5/10".into())], 4);
        assert_eq!(
            String::from_utf8(head).unwrap(),
            "HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nConnection: close\r\nContent-Range: bytes 2-5/10\r\n\r\n"
        );
    }
}
