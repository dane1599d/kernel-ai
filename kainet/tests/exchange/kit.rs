//! The scripted servers and bodies `tests/exchange.rs` and
//! `tests/exchange_coding.rs` share.

#![allow(dead_code)]

use kainet::http::{Answer, Body, Connect, Fetch, HttpError, Limits, Request, Stream, Url, Verb};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Write;
use std::rc::Rc;

/// What each connection was sent, by connection number.
pub type Sent = Rc<RefCell<Vec<Vec<u8>>>>;

/// One connection: each reply is read once the request before it was sent,
/// `step` bytes at a time, and once they run out it ends cleanly (or, with
/// `cut`, fails), as a server closing an idle connection does.
pub struct Conn {
    pub id: usize,
    pub replies: VecDeque<Vec<u8>>,
    pub out: Vec<u8>,
    pub at: usize,
    pub step: usize,
    pub cut: bool,
    pub sent: Sent,
}

impl Stream for Conn {
    fn send(&mut self, bytes: &[u8]) -> Result<(), HttpError> {
        self.sent.borrow_mut()[self.id].extend_from_slice(bytes);
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, HttpError> {
        if self.at == self.out.len() {
            match self.replies.pop_front() {
                Some(next) => (self.out, self.at) = (next, 0),
                None if self.cut => return Err(HttpError::Truncated),
                None => return Ok(0),
            }
        }
        let n = buf.len().min(self.step).min(self.out.len() - self.at);
        buf[..n].copy_from_slice(&self.out[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// Each new connection takes the next script: its replies in turn.
pub struct Server {
    pub scripts: VecDeque<Vec<Vec<u8>>>,
    pub step: usize,
    pub cut: bool,
    pub sent: Sent,
}

impl Server {
    pub fn new(scripts: &[&[&[u8]]]) -> Self {
        Self {
            scripts: scripts
                .iter()
                .map(|s| s.iter().map(|r| r.to_vec()).collect())
                .collect(),
            step: usize::MAX,
            cut: false,
            sent: Sent::default(),
        }
    }

    pub fn owned(scripts: Vec<Vec<Vec<u8>>>) -> Self {
        Self {
            scripts: scripts.into(),
            step: usize::MAX,
            cut: false,
            sent: Sent::default(),
        }
    }

    pub fn stepped(mut self, step: usize) -> Self {
        self.step = step;
        self
    }

    /// What each connection was sent, as text.
    pub fn sent(&self) -> Vec<String> {
        self.sent
            .borrow()
            .iter()
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect()
    }
}

impl Connect for Server {
    type Stream = Conn;

    fn connect(&mut self, _: &Url) -> Result<Conn, HttpError> {
        let replies = self
            .scripts
            .pop_front()
            .ok_or(HttpError::Connect("no more connections"))?;
        let mut sent = self.sent.borrow_mut();
        sent.push(Vec::new());
        Ok(Conn {
            id: sent.len() - 1,
            replies: replies.into(),
            out: Vec::new(),
            at: 0,
            step: self.step,
            cut: self.cut,
            sent: Rc::clone(&self.sent),
        })
    }
}

pub fn anyone(_: &str, _: u16) -> bool {
    true
}

pub fn zero() -> u64 {
    0
}

pub fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

/// A request for `verb` on `at` with no headers and no body, decoding.
pub fn ask<'a>(verb: Verb, at: &'a Url) -> Request<'a> {
    Request {
        verb,
        url: at,
        headers: &[],
        body: Body::Empty,
        decode: true,
    }
}

/// `request` exchanged with `connect` under `max_body`: the answer and the
/// body that reached the sink.
pub fn run(
    connect: &mut impl Connect,
    request: Request<'_>,
    max_body: u64,
) -> (Result<Answer, HttpError>, Vec<u8>) {
    let fetch = Fetch {
        limits: Limits::new(max_body, u64::MAX),
        allow: &anyone,
        clock: &zero,
    };
    let mut body = Vec::new();
    let answer = fetch.exchange(connect, request, &mut |bytes| {
        body.extend_from_slice(bytes);
        Ok(())
    });
    (answer, body)
}

/// A response of `status` with `headers` and `body` by length.
pub fn reply(status: &str, headers: &[&str], body: &[u8]) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
    for header in headers {
        head.push_str(header);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

/// `body` in chunks of `size` bytes.
pub fn chunked(status: &str, headers: &[&str], body: &[u8], size: usize) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status}\r\nTransfer-Encoding: chunked\r\n");
    for header in headers {
        out.push_str(header);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    let mut bytes = out.into_bytes();
    for piece in body.chunks(size) {
        bytes.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
        bytes.extend_from_slice(piece);
        bytes.extend_from_slice(b"\r\n");
    }
    bytes.extend_from_slice(b"0\r\n\r\n");
    bytes
}

/// Bytes that do not repeat soon, so codings cannot shrink them much.
pub fn noise(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2_654_435_761).max(1);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

/// Text that codes well: a page's worth of repeated words.
pub fn page(len: usize) -> Vec<u8> {
    let words = b"the quick brown fox jumps over the lazy dog; ";
    words.iter().cycle().take(len).copied().collect()
}

pub fn gzip(body: &[u8]) -> Vec<u8> {
    let mut coder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    coder.write_all(body).unwrap();
    coder.finish().unwrap()
}

pub fn brotli(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut coder = brotli::CompressorWriter::new(&mut out, 4096, 5, 22);
        coder.write_all(body).unwrap();
    }
    out
}

pub const MIB: usize = 1 << 20;
