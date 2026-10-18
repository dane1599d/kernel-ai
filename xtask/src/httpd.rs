//! The server side of one HTTP/1.1 exchange per connection, for the test stand-ins
//! the guest reaches at 10.0.2.2: the mock LLM and the build service.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// Longest request line or header line read.
const MAX_LINE: usize = 8 * 1024;
/// Most header lines read.
const MAX_HEADERS: usize = 64;
/// How long a whole request may take to arrive, and a response to be taken.
const REQUEST_LIMIT: Duration = Duration::from_secs(30);
/// How long the request line and headers may take: a caller that is not allowed
/// in is turned away before its body, and a slow one soon.
const HEAD_LIMIT: Duration = Duration::from_secs(5);

/// Reads from a stream with a deadline for the whole request, not each read, so a
/// client sending a byte at a time cannot hold a handler for ever.
struct Deadline<'a> {
    stream: &'a TcpStream,
    until: Instant,
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the request took too long",
            ));
        }
        self.stream.set_read_timeout(Some(left))?;
        let mut stream = self.stream;
        stream.read(buf)
    }
}

pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug)]
pub enum HttpError {
    Io(io::Error),
    /// Not HTTP as this reader takes it; answer 400.
    Malformed(&'static str),
    /// A body over the limit; answer 413.
    TooLarge,
    /// The headers say the caller may not ask; answer 403.
    Forbidden,
}

impl From<io::Error> for HttpError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn read_line(reader: &mut impl BufRead) -> Result<String, HttpError> {
    let mut line = Vec::new();
    reader
        .take(MAX_LINE as u64 + 1)
        .read_until(b'\n', &mut line)?;
    if line.len() > MAX_LINE {
        return Err(HttpError::Malformed("a line is too long"));
    }
    let line = String::from_utf8(line).map_err(|_| HttpError::Malformed("a line is not text"))?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// Reads one request with a body of at most `max_body` bytes, if `allowed` says
/// yes to its headers.
pub fn read_request(
    stream: &TcpStream,
    max_body: usize,
    allowed: impl Fn(&Request) -> bool,
) -> Result<Request, HttpError> {
    read_request_within(stream, max_body, (HEAD_LIMIT, REQUEST_LIMIT), allowed)
}

fn read_request_within(
    stream: &TcpStream,
    max_body: usize,
    (head, limit): (Duration, Duration),
    allowed: impl Fn(&Request) -> bool,
) -> Result<Request, HttpError> {
    stream.set_write_timeout(Some(limit))?;
    let started = Instant::now();
    let mut reader = BufReader::new(Deadline {
        stream,
        until: started + head,
    });
    let request_line = read_line(&mut reader)?;
    let mut parts = request_line.split(' ');
    let (Some(method), Some(path), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(HttpError::Malformed("not a request line"));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(HttpError::Malformed("not HTTP/1.x"));
    }
    let mut headers = Vec::new();
    loop {
        let line = read_line(&mut reader)?;
        if line.is_empty() {
            break;
        }
        if headers.len() == MAX_HEADERS {
            return Err(HttpError::Malformed("too many headers"));
        }
        let (name, value) = line
            .split_once(':')
            .ok_or(HttpError::Malformed("a header has no colon"))?;
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }
    let mut request = Request {
        method: method.to_string(),
        path: path.to_string(),
        headers,
        body: Vec::new(),
    };
    if !allowed(&request) {
        return Err(HttpError::Forbidden);
    }
    reader.get_mut().until = started + limit;
    let length = match request.header("content-length") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| HttpError::Malformed("bad Content-Length"))?,
        None => 0,
    };
    if length > max_body {
        return Err(HttpError::TooLarge);
    }
    request.body = vec![0; length];
    reader.read_exact(&mut request.body)?;
    Ok(request)
}

/// Writes one response and ends the exchange.
pub fn respond(
    mut stream: &TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn serve_one(raw: &'static [u8], max: usize) -> Result<Request, HttpError> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream.write_all(raw).unwrap();
            stream
        });
        let (stream, _) = listener.accept().unwrap();
        let request = read_request(&stream, max, |_| true);
        drop(client.join().unwrap());
        request
    }

    #[test]
    fn reads_the_request_line_headers_and_body() {
        let request = serve_one(
            b"POST /build HTTP/1.1\r\nX-Kai-Token: abc\r\nContent-Length: 3\r\n\r\nxyz",
            10,
        )
        .unwrap();
        assert_eq!(
            (request.method.as_str(), request.path.as_str()),
            ("POST", "/build")
        );
        assert_eq!(request.header("x-kai-token"), Some("abc"));
        assert_eq!(request.body, b"xyz");
    }

    #[test]
    fn a_request_that_trickles_in_runs_out_of_time() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            for byte in b"GET /health HTTP/1.1\r\n" {
                if stream.write_all(&[*byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(40));
            }
        });
        let (stream, _) = listener.accept().unwrap();
        let started = Instant::now();
        let limits = (Duration::from_millis(300), Duration::from_secs(1));
        let request = read_request_within(&stream, 10, limits, |_| true);
        assert!(matches!(request, Err(HttpError::Io(_))));
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(stream);
        client.join().unwrap();
    }

    #[test]
    fn refuses_oversized_and_malformed_requests() {
        let big = serve_one(b"POST / HTTP/1.1\r\nContent-Length: 11\r\n\r\n", 10);
        assert!(matches!(big, Err(HttpError::TooLarge)));
        let bad = serve_one(b"GARBAGE\r\n\r\n", 10);
        assert!(matches!(bad, Err(HttpError::Malformed(_))));
        let colon = serve_one(b"GET / HTTP/1.1\r\nno colon\r\n\r\n", 10);
        assert!(matches!(colon, Err(HttpError::Malformed(_))));
    }
}
