//! Minimal HTTP/1.1 client side: one request per connection (`Connection: close`).

use alloc::format;
use alloc::vec::Vec;

/// Upper bound on a response the kernel will buffer.
pub const MAX_RESPONSE: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// More bytes are needed.
    Incomplete,
    Malformed(&'static str),
    TooLarge,
}

/// Builds a POST request with a JSON body.
pub fn post_json(host: &str, path: &str, api_key: Option<&str>, body: &[u8]) -> Vec<u8> {
    let auth = api_key
        .map(|key| format!("Authorization: Bearer {key}\r\n"))
        .unwrap_or_default();
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: kernel-ai\r\nAccept: application/json\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n{auth}Connection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    request.extend_from_slice(body);
    request
}

/// Builds a GET request.
pub fn get(host: &str, path: &str) -> Vec<u8> {
    format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: kernel-ai\r\nConnection: close\r\n\r\n"
    )
    .into_bytes()
}

/// Parses a response from the bytes received so far. `closed` says the server has
/// closed the connection, which ends a body that has no length.
pub fn parse_response(buf: &[u8], closed: bool) -> Result<Response, ParseError> {
    if buf.len() > MAX_RESPONSE {
        return Err(ParseError::TooLarge);
    }
    let Some(header_end) = find(buf, b"\r\n\r\n") else {
        return Err(if closed {
            ParseError::Malformed("connection closed before the headers ended")
        } else {
            ParseError::Incomplete
        });
    };
    let head = core::str::from_utf8(&buf[..header_end])
        .map_err(|_| ParseError::Malformed("headers are not UTF-8"))?;
    let mut lines = head.split("\r\n");
    let status = parse_status_line(lines.next().unwrap_or(""))?;
    let mut content_length = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return Err(ParseError::Malformed("header line without a colon"));
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            let length = value
                .parse::<usize>()
                .map_err(|_| ParseError::Malformed("bad Content-Length"))?;
            content_length = Some(length);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.to_ascii_lowercase().contains("chunked");
        }
    }
    let rest = &buf[header_end + 4..];
    let body = if chunked {
        decode_chunked(rest, closed)?
    } else if let Some(length) = content_length {
        if rest.len() < length {
            return Err(if closed {
                ParseError::Malformed("connection closed before the body ended")
            } else {
                ParseError::Incomplete
            });
        }
        rest[..length].to_vec()
    } else if closed {
        rest.to_vec()
    } else {
        return Err(ParseError::Incomplete);
    };
    Ok(Response { status, body })
}

fn parse_status_line(line: &str) -> Result<u16, ParseError> {
    let mut parts = line.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/1.") {
        return Err(ParseError::Malformed("not an HTTP/1.x status line"));
    }
    parts
        .next()
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or(ParseError::Malformed("bad status code"))
}

fn decode_chunked(mut rest: &[u8], closed: bool) -> Result<Vec<u8>, ParseError> {
    let incomplete = || {
        if closed {
            ParseError::Malformed("connection closed inside a chunk")
        } else {
            ParseError::Incomplete
        }
    };
    let mut body = Vec::new();
    loop {
        let line_end = find(rest, b"\r\n").ok_or_else(incomplete)?;
        let size_text = core::str::from_utf8(&rest[..line_end])
            .map_err(|_| ParseError::Malformed("chunk size is not text"))?;
        let size_text = size_text.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| ParseError::Malformed("bad chunk size"))?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Ok(body);
        }
        if rest.len() < size + 2 {
            return Err(incomplete());
        }
        body.extend_from_slice(&rest[..size]);
        if &rest[size..size + 2] != b"\r\n" {
            return Err(ParseError::Malformed("chunk not followed by CRLF"));
        }
        rest = &rest[size + 2..];
        if body.len() > MAX_RESPONSE {
            return Err(ParseError::TooLarge);
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    #[test]
    fn builds_post_with_length_and_optional_key() {
        let request =
            String::from_utf8(post_json("10.0.2.2:8080", "/v1/chat", Some("k"), b"{}")).unwrap();
        assert!(request.starts_with("POST /v1/chat HTTP/1.1\r\nHost: 10.0.2.2:8080\r\n"));
        assert!(request.contains("Content-Length: 2\r\n"));
        assert!(request.contains("Authorization: Bearer k\r\n"));
        assert!(request.ends_with("\r\n\r\n{}"));
        let request = String::from_utf8(post_json("h", "/", None, b"")).unwrap();
        assert!(!request.contains("Authorization"));
    }

    #[test]
    fn parses_content_length_and_waits_for_the_rest() {
        let full = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nX: y\r\n\r\nhello";
        assert_eq!(
            parse_response(full, false),
            Ok(Response {
                status: 200,
                body: b"hello".to_vec()
            })
        );
        assert_eq!(
            parse_response(&full[..full.len() - 1], false),
            Err(ParseError::Incomplete)
        );
        assert!(matches!(
            parse_response(&full[..full.len() - 1], true),
            Err(ParseError::Malformed(_))
        ));
        assert_eq!(
            parse_response(b"HTTP/1.1 200", false),
            Err(ParseError::Incomplete)
        );
    }

    #[test]
    fn decodes_chunked_bodies_in_pieces() {
        let full = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n4\r\nWiki\r\n5;ext=1\r\npedia\r\n0\r\n\r\n";
        assert_eq!(
            parse_response(full, false).unwrap().body,
            b"Wikipedia".to_vec()
        );
        for cut in 50..full.len() - 4 {
            assert_eq!(
                parse_response(&full[..cut], false),
                Err(ParseError::Incomplete),
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn body_without_length_ends_at_close() {
        let raw = b"HTTP/1.0 500 Internal\r\n\r\noops";
        assert_eq!(parse_response(raw, false), Err(ParseError::Incomplete));
        assert_eq!(
            parse_response(raw, true).unwrap(),
            Response {
                status: 500,
                body: b"oops".to_vec()
            }
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(
            parse_response(b"SSH-2.0\r\n\r\n", false),
            Err(ParseError::Malformed(_))
        ));
        assert!(matches!(
            parse_response(b"HTTP/1.1 abc\r\n\r\n", false),
            Err(ParseError::Malformed(_))
        ));
        assert!(matches!(
            parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: x\r\n\r\n", false),
            Err(ParseError::Malformed(_))
        ));
        let big = alloc::vec![b'a'; MAX_RESPONSE + 1];
        assert_eq!(parse_response(&big, false), Err(ParseError::TooLarge));
    }
}
