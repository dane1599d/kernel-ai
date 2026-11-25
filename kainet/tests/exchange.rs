//! `Fetch::exchange` (plan P6 F-4, the host half) against scripted servers:
//! request methods, headers and bodies of any size; every response header;
//! bodies streamed past 64 KiB; gzip and brotli undone in any pieces; kept
//! connections; and hostile heads, headers and codings refused.

use kainet::http::exchange::{self, MAX_REQUEST_HEADERS};
use kainet::http::{
    Answer, Body, Coding, Connect, Fetch, Framing, HttpError, Keep, Limits, Request, Stream, Url,
    Verb,
};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Write;
use std::rc::Rc;

/// What each connection was sent, by connection number.
type Sent = Rc<RefCell<Vec<Vec<u8>>>>;

/// One connection: each reply is read once the request before it was sent,
/// `step` bytes at a time, and once they run out it ends cleanly (or, with
/// `cut`, fails), as a server closing an idle connection does.
struct Conn {
    id: usize,
    replies: VecDeque<Vec<u8>>,
    out: Vec<u8>,
    at: usize,
    step: usize,
    cut: bool,
    sent: Sent,
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
struct Server {
    scripts: VecDeque<Vec<Vec<u8>>>,
    step: usize,
    cut: bool,
    sent: Sent,
}

impl Server {
    fn new(scripts: &[&[&[u8]]]) -> Self {
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

    fn owned(scripts: Vec<Vec<Vec<u8>>>) -> Self {
        Self {
            scripts: scripts.into(),
            step: usize::MAX,
            cut: false,
            sent: Sent::default(),
        }
    }

    fn stepped(mut self, step: usize) -> Self {
        self.step = step;
        self
    }

    /// What each connection was sent, as text.
    fn sent(&self) -> Vec<String> {
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

fn anyone(_: &str, _: u16) -> bool {
    true
}

fn zero() -> u64 {
    0
}

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

/// A request for `verb` on `at` with no headers and no body, decoding.
fn ask<'a>(verb: Verb, at: &'a Url) -> Request<'a> {
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
fn run(
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
fn reply(status: &str, headers: &[&str], body: &[u8]) -> Vec<u8> {
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
fn chunked(status: &str, headers: &[&str], body: &[u8], size: usize) -> Vec<u8> {
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
fn noise(len: usize, seed: u32) -> Vec<u8> {
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
fn page(len: usize) -> Vec<u8> {
    let words = b"the quick brown fox jumps over the lazy dog; ";
    words.iter().cycle().take(len).copied().collect()
}

fn gzip(body: &[u8]) -> Vec<u8> {
    let mut coder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    coder.write_all(body).unwrap();
    coder.finish().unwrap()
}

fn brotli(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut coder = brotli::CompressorWriter::new(&mut out, 4096, 5, 22);
        coder.write_all(body).unwrap();
    }
    out
}

const MIB: usize = 1 << 20;

#[test]
fn every_verb_sends_its_line_the_clients_headers_then_the_callers() {
    let at = url("https://a.test:8443/p?q=1");
    for verb in [
        Verb::Get,
        Verb::Head,
        Verb::Post,
        Verb::Put,
        Verb::Patch,
        Verb::Delete,
        Verb::Options,
    ] {
        let headers = [("Accept", "text/html"), ("X-Two", "a, b")];
        let request = Request {
            headers: &headers,
            ..ask(verb, &at)
        };
        let head = String::from_utf8(exchange::request_head(&request, false).unwrap()).unwrap();
        let length = match verb {
            Verb::Post | Verb::Put | Verb::Patch => "Content-Length: 0\r\n",
            _ => "",
        };
        assert_eq!(
            head,
            format!(
                "{} /p?q=1 HTTP/1.1\r\nHost: a.test:8443\r\nUser-Agent: kai\r\n\
                 Accept-Encoding: gzip, br\r\nConnection: close\r\nAccept: text/html\r\n\
                 X-Two: a, b\r\n{length}\r\n",
                verb.name()
            )
        );
    }
    // The caller's User-Agent replaces the client's; no decoding asks for
    // identity; a kept connection asks to be kept.
    let headers = [("user-agent", "Mozilla/5.0 (Kai)")];
    let request = Request {
        headers: &headers,
        decode: false,
        ..ask(Verb::Get, &at)
    };
    let head = String::from_utf8(exchange::request_head(&request, true).unwrap()).unwrap();
    assert!(!head.contains("User-Agent: kai"), "{head}");
    assert!(head.contains("user-agent: Mozilla/5.0 (Kai)\r\n"), "{head}");
    assert!(head.contains("Accept-Encoding: identity\r\nConnection: keep-alive\r\n"));
}

#[test]
fn a_header_that_could_end_a_line_or_that_the_client_sets_is_refused() {
    let at = url("https://a.test/");
    let long_name = "x".repeat(exchange::MAX_NAME + 1);
    let long_value = "v".repeat(exchange::MAX_VALUE + 1);
    for (name, value) in [
        ("X-A", "one\r\nX-Injected: two"),
        ("X-A", "one\nGET / HTTP/1.1"),
        ("X-A", "nul\0"),
        ("X-A", "caf\u{e9}"),
        ("X-A", " padded"),
        ("X-A", "padded\t"),
        ("X A", "v"),
        ("X:A", "v"),
        ("", "v"),
        ("X-A\r\n", "v"),
        (long_name.as_str(), "v"),
        ("X-A", long_value.as_str()),
        ("Host", "b.test"),
        ("content-length", "5"),
        ("Transfer-Encoding", "chunked"),
        ("Connection", "upgrade"),
        ("Upgrade", "websocket"),
        ("TE", "trailers"),
        ("Expect", "100-continue"),
        ("Accept-Encoding", "zstd"),
    ] {
        let headers = [(name, value)];
        let request = Request {
            headers: &headers,
            ..ask(Verb::Get, &at)
        };
        assert!(
            matches!(
                exchange::request_head(&request, false),
                Err(HttpError::Url(_))
            ),
            "{name:?}: {value:?}"
        );
        // Nothing reaches the server.
        let mut server = Server::new(&[&[b"HTTP/1.1 200 OK\r\n\r\n"]]);
        let request = Request {
            headers: &headers,
            ..ask(Verb::Post, &at)
        };
        assert!(matches!(
            run(&mut server, request, 1024).0,
            Err(HttpError::Url(_))
        ));
        assert!(server.sent().is_empty());
    }
    // An empty value is a value.
    assert_eq!(exchange::check_header("X-Empty", ""), Ok(()));
    let many: Vec<(&str, &str)> = (0..=MAX_REQUEST_HEADERS).map(|_| ("X-N", "1")).collect();
    let request = Request {
        headers: &many,
        ..ask(Verb::Get, &at)
    };
    assert!(exchange::request_head(&request, false).is_err());
    // A GET or HEAD carries no body.
    for verb in [Verb::Get, Verb::Head] {
        let request = Request {
            body: Body::Bytes(b"x"),
            ..ask(verb, &at)
        };
        assert!(exchange::request_head(&request, false).is_err());
    }
}

/// A request body far past 64 KiB goes whole, by length or in chunks, as it
/// is read; the server's answer comes back.
#[test]
fn a_request_body_of_megabytes_streams_by_length_or_in_chunks() {
    let at = url("https://a.test/upload");
    let body = noise(3 * MIB + 17, 1);
    for (length, framing) in [
        (Some(body.len() as u64), "Content-Length: 3145745\r\n"),
        (None, "Transfer-Encoding: chunked\r\n"),
    ] {
        let mut server = Server::new(&[&[b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nok"]]);
        let mut at_byte = 0;
        // The caller's read gives odd amounts, never more than asked.
        let mut read = |buf: &mut [u8]| {
            let n = buf.len().min(5000).min(body.len() - at_byte);
            buf[..n].copy_from_slice(&body[at_byte..at_byte + n]);
            at_byte += n;
            Ok(n)
        };
        let request = Request {
            body: Body::Stream {
                length,
                read: &mut read,
            },
            ..ask(Verb::Put, &at)
        };
        let (answer, got) = run(&mut server, request, 1024);
        assert_eq!(answer.unwrap().head.status, 201);
        assert_eq!(got, b"ok");
        let sent = server.sent.borrow()[0].clone();
        let end = sent.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        let head = String::from_utf8_lossy(&sent[..end]);
        assert!(head.contains(framing), "{head}");
        let arrived = match length {
            Some(_) => sent[end..].to_vec(),
            None => unchunk(&sent[end..]),
        };
        assert!(arrived == body, "the body arrived changed");
    }
    // Bytes of any size, by length.
    let big = noise(MIB, 2);
    let mut server = Server::new(&[&[b"HTTP/1.1 204 No Content\r\n\r\n"]]);
    let request = Request {
        body: Body::Bytes(&big),
        ..ask(Verb::Post, &at)
    };
    assert_eq!(run(&mut server, request, 0).0.unwrap().head.status, 204);
    let sent = server.sent.borrow()[0].clone();
    assert!(sent.ends_with(&big));
    assert!(String::from_utf8_lossy(&sent).contains("Content-Length: 1048576\r\n"));
}

/// A chunked body's data.
fn unchunk(mut bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let line = bytes.windows(2).position(|w| w == b"\r\n").unwrap();
        let size = usize::from_str_radix(std::str::from_utf8(&bytes[..line]).unwrap(), 16).unwrap();
        bytes = &bytes[line + 2..];
        if size == 0 {
            assert_eq!(bytes, b"\r\n");
            return out;
        }
        out.extend_from_slice(&bytes[..size]);
        assert_eq!(&bytes[size..size + 2], b"\r\n");
        bytes = &bytes[size + 2..];
    }
}

/// A streamed body whose read ends early, or gives more than its length, is
/// refused; for a PUT, once its head went out, that is a cut.
#[test]
fn a_streamed_body_that_is_not_its_length_is_a_cut() {
    let at = url("https://a.test/upload");
    for (length, give) in [(10u64, 9usize), (10, 11)] {
        let mut server = Server::new(&[&[b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"]]);
        let mut left = give;
        let mut read = |buf: &mut [u8]| {
            let n = buf.len().min(left);
            left -= n;
            Ok(n)
        };
        let request = Request {
            body: Body::Stream {
                length: Some(length),
                read: &mut read,
            },
            ..ask(Verb::Put, &at)
        };
        assert_eq!(run(&mut server, request, 1024).0, Err(HttpError::Cut));
        // Never more than the length went out.
        let sent = server.sent.borrow()[0].clone();
        let end = sent.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        assert!(sent.len() - end <= 10);
    }
}

/// Every header line comes back in order, a value that is not UTF-8 among
/// them, and a body of any status, far past 64 KiB, by length or chunks, in
/// any pieces.
#[test]
fn every_header_and_a_body_of_megabytes_come_back_in_any_pieces() {
    let at = url("https://a.test/big");
    let body = noise(2 * MIB + 3, 3);
    let headers = [
        "Set-Cookie: a=1; Secure",
        "Set-Cookie: b=2",
        "X-Latin: caf\u{e9}",
        "Content-Type: text/html",
    ];
    let mut by_length = reply("404 Not Found", &headers, &body);
    // A Latin-1 byte, not UTF-8, as old servers send.
    let at_e = by_length
        .windows(5)
        .position(|w| w == "caf\u{e9}".as_bytes())
        .unwrap();
    by_length.splice(at_e + 3..at_e + 5, [0xE9]);
    let by_chunks = chunked("404 Not Found", &headers[..2], &body, 70_000);
    for (wire, kept) in [(by_length, 5), (by_chunks, 3)] {
        for step in [1 << 20, 4093, 1] {
            if step == 1 && wire.len() > 300_000 {
                // A byte at a time over a prefix only; the rest is the same code.
                continue;
            }
            let mut server = Server::owned(vec![vec![wire.clone()]]).stepped(step);
            let (answer, got) = run(&mut server, ask(Verb::Get, &at), 4 * MIB as u64);
            let answer = answer.unwrap();
            assert_eq!(answer.head.status, 404);
            assert_eq!(answer.received, body.len() as u64);
            assert!(got == body, "step {step}");
            assert_eq!(answer.headers.len(), kept);
            assert_eq!(
                answer.headers[1],
                ("Set-Cookie".into(), b"a=1; Secure".to_vec())
            );
            assert_eq!(answer.headers[2], ("Set-Cookie".into(), b"b=2".to_vec()));
            if kept == 5 {
                assert_eq!(answer.headers[3].1, b"caf\xE9");
                assert_eq!(answer.head.content_type.as_deref(), Some("text/html"));
            }
            assert_eq!(answer.coding, Coding::Identity);
        }
    }
    // A byte at a time, for a body past 64 KiB.
    let small = noise(70_000, 4);
    let mut server = Server::owned(vec![vec![chunked("200 OK", &[], &small, 999)]]).stepped(1);
    let (answer, got) = run(&mut server, ask(Verb::Get, &at), 1 << 20);
    assert_eq!(answer.unwrap().received, 70_000);
    assert!(got == small);
}

/// gzip, with a name, a comment, an extra field and a header CRC, and brotli,
/// are undone in any pieces; what reaches the sink is the page.
#[test]
fn gzip_and_brotli_bodies_are_decoded_in_any_pieces() {
    let at = url("https://a.test/page");
    let text = page(MIB + 5);
    let mut fancy = gzip_with_fields(&text);
    let plain = gzip(&text);
    let br = brotli(&text);
    for (coding, coded, name) in [
        (Coding::Gzip, plain.clone(), "gzip"),
        (Coding::Gzip, plain, "x-gzip"),
        (Coding::Gzip, std::mem::take(&mut fancy), "GZIP"),
        (Coding::Brotli, br, "br"),
    ] {
        assert!(coded.len() < text.len() / 10, "{name} codes the page");
        let header = format!("Content-Encoding: {name}");
        for step in [1 << 20, 1000, 7] {
            for wire in [
                reply("200 OK", &[&header], &coded),
                chunked("200 OK", &[&header], &coded, 333),
            ] {
                let mut server = Server::owned(vec![vec![wire]]).stepped(step);
                let (answer, got) = run(&mut server, ask(Verb::Get, &at), 2 * MIB as u64);
                let answer = answer.unwrap_or_else(|e| panic!("{name} step {step}: {e:?}"));
                assert_eq!(answer.coding, coding);
                assert_eq!(answer.received, text.len() as u64);
                assert_eq!(answer.wire, coded.len() as u64);
                assert!(got == text, "{name} step {step}");
            }
        }
    }
    // Asked for and identity, or no coding: passed as it is.
    let mut server = Server::new(&[&[
        b"HTTP/1.1 200 OK\r\nContent-Encoding: identity\r\nContent-Length: 2\r\n\r\nhi",
    ]]);
    assert_eq!(run(&mut server, ask(Verb::Get, &at), 10).1, b"hi");
    // An empty coded body is empty, as browsers take it.
    let mut server = Server::new(&[&[
        b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 0\r\n\r\n",
    ]]);
    let (answer, got) = run(&mut server, ask(Verb::Get, &at), 10);
    assert_eq!((answer.unwrap().received, got.len()), (0, 0));
}

/// A gzip member with every optional header field and a header CRC.
fn gzip_with_fields(body: &[u8]) -> Vec<u8> {
    let plain = gzip(body);
    // FHCRC | FEXTRA | FNAME | FCOMMENT, the fixed bytes otherwise flate2's.
    let mut header = plain[..10].to_vec();
    header[3] = 0x02 | 0x04 | 0x08 | 0x10;
    header.extend_from_slice(&5u16.to_le_bytes());
    header.extend_from_slice(b"KA\x01\x00z");
    header.extend_from_slice(b"page.html\0");
    header.extend_from_slice(b"a comment\0");
    let crc = crc32(&header) as u16;
    header.extend_from_slice(&crc.to_le_bytes());
    header.extend_from_slice(&plain[10..]);
    header
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut gz = flate2::Crc::new();
    gz.update(bytes);
    gz.sum()
}

/// A small coded body that decodes without end stops at the limit on decoded
/// bytes, holding no more than its buffers; broken codings are refused.
#[test]
fn hostile_codings_are_refused_and_a_bomb_stops_at_the_limit() {
    let at = url("https://a.test/bomb");
    let zeros = vec![0u8; 16 * MIB];
    for (name, coded) in [("gzip", gzip(&zeros)), ("br", brotli(&zeros))] {
        assert!(
            coded.len() < 200 * 1024,
            "{name} is a bomb: {}",
            coded.len()
        );
        let header = format!("Content-Encoding: {name}");
        let mut server = Server::owned(vec![vec![reply("200 OK", &[&header], &coded)]]);
        let mut passed = 0u64;
        let fetch = Fetch {
            limits: Limits::new(MIB as u64, u64::MAX),
            allow: &anyone,
            clock: &zero,
        };
        let got = fetch.exchange(&mut server, ask(Verb::Get, &at), &mut |bytes| {
            passed += bytes.len() as u64;
            Ok(())
        });
        assert_eq!(got, Err(HttpError::TooLarge), "{name}");
        assert!(passed <= MIB as u64, "{name}: {passed}");
    }
    let text = page(100_000);
    let good = gzip(&text);
    let mut bad_crc = good.clone();
    let n = bad_crc.len();
    bad_crc[n - 8] ^= 1;
    let mut bad_size = good.clone();
    bad_size[n - 1] ^= 1;
    let mut bad_deflate = good.clone();
    bad_deflate[12] ^= 0xFF;
    let mut reserved = good.clone();
    reserved[3] = 0x20;
    let mut not_deflate = good.clone();
    not_deflate[2] = 7;
    let mut bad_header_crc = gzip_with_fields(&text);
    let crc_at = 10 + 2 + 5 + 10 + 10;
    bad_header_crc[crc_at] ^= 1;
    let mut trailing = good.clone();
    trailing.extend_from_slice(&good);
    let mut endless_name = good[..10].to_vec();
    endless_name[3] = 0x08;
    endless_name.extend(std::iter::repeat_n(b'n', 5000));
    let br = brotli(&text);
    let mut br_trailing = br.clone();
    br_trailing.extend_from_slice(b"more");
    for (name, coding, body) in [
        ("a bad CRC", "gzip", bad_crc),
        ("a bad size", "gzip", bad_size),
        ("broken deflate", "gzip", bad_deflate),
        ("a reserved flag", "gzip", reserved),
        ("a method not deflate", "gzip", not_deflate),
        ("a bad header CRC", "gzip", bad_header_crc),
        ("a second member", "gzip", trailing),
        ("a name without end", "gzip", endless_name),
        ("cut short", "gzip", good[..good.len() - 3].to_vec()),
        ("not gzip", "gzip", text[..100].to_vec()),
        ("not brotli", "br", text[..100].to_vec()),
        ("brotli cut short", "br", br[..br.len() / 2].to_vec()),
        ("bytes after brotli", "br", br_trailing),
    ] {
        let header = format!("Content-Encoding: {coding}");
        let mut server = Server::owned(vec![vec![reply("200 OK", &[&header], &body)]]);
        let got = run(&mut server, ask(Verb::Get, &at), 4 * MIB as u64).0;
        assert!(matches!(got, Err(HttpError::Body(_))), "{name}: {got:?}");
    }
    // Codings it did not ask for, more than one, or ones it does not know.
    for (decode, header) in [
        (false, "Content-Encoding: gzip"),
        (false, "Content-Encoding: br"),
        (true, "Content-Encoding: deflate"),
        (true, "Content-Encoding: compress"),
        (true, "Content-Encoding: zstd"),
        (true, "Content-Encoding: gzip, br"),
        (true, "Content-Encoding: gzip\r\nContent-Encoding: gzip"),
    ] {
        let mut server = Server::owned(vec![vec![reply("200 OK", &[header], &good)]]);
        let request = Request {
            decode,
            ..ask(Verb::Get, &at)
        };
        let got = run(&mut server, request, MIB as u64).0;
        assert!(matches!(got, Err(HttpError::Head(_))), "{header}: {got:?}");
    }
}

/// Hostile heads are refused as `Fetch::run` refuses them, and a head or a
/// body on the wire over its limit is too large.
#[test]
fn hostile_heads_and_bodies_over_their_limits_are_refused() {
    let at = url("https://a.test/");
    for wire in [
        &b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\nhello"[..],
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Length: \xff\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nLocation: \xff\r\n\r\n",
        b"HTTP/1.1 101 Switching Protocols\r\n\r\n",
        b"HTTP/1.0 200 OK\r\n\r\n",
        b"NOT HTTP\r\n\r\n",
    ] {
        let mut server = Server::new(&[&[wire]]);
        let got = run(&mut server, ask(Verb::Get, &at), 100).0;
        assert!(
            matches!(got, Err(HttpError::Head(_))),
            "{:?}: {got:?}",
            String::from_utf8_lossy(wire)
        );
    }
    let mut lines = String::from("HTTP/1.1 200 OK\r\n");
    for n in 0..=kainet::http::head::MAX_KEPT_HEADERS {
        lines.push_str(&format!("X-{n}: v\r\n"));
    }
    lines.push_str("\r\n");
    let mut server = Server::owned(vec![vec![lines.into_bytes()]]);
    assert!(matches!(
        run(&mut server, ask(Verb::Get, &at), 100).0,
        Err(HttpError::Head(_))
    ));
    // A head past max_head, and bodies past max_body on the wire.
    let long = format!("HTTP/1.1 200 OK\r\nX: {}\r\n\r\n", "a".repeat(20_000));
    let mut server = Server::owned(vec![vec![long.into_bytes()]]);
    assert_eq!(
        run(&mut server, ask(Verb::Get, &at), 100).0,
        Err(HttpError::TooLarge)
    );
    for wire in [
        reply("200 OK", &[], &[1; 101]),
        chunked("200 OK", &[], &[1; 101], 50),
    ] {
        let mut server = Server::owned(vec![vec![wire]]);
        assert_eq!(
            run(&mut server, ask(Verb::Get, &at), 100).0,
            Err(HttpError::TooLarge)
        );
    }
    // Cut inside a body: truncated.
    let mut cut = reply("200 OK", &[], &[1; 50]);
    cut.truncate(cut.len() - 1);
    let mut server = Server::owned(vec![vec![cut]]);
    assert_eq!(
        run(&mut server, ask(Verb::Get, &at), 100).0,
        Err(HttpError::Truncated)
    );
}

/// Exchanges over a `Keep` reuse one connection while each answer is read
/// whole; an unsafe one never takes a kept connection but gives its own
/// back; one that says it closes, or is read until close, is not kept.
#[test]
fn kept_connections_carry_the_next_exchange() {
    let at = url("https://a.test/");
    let ok = reply("200 OK", &[], b"one");
    let gz = reply("200 OK", &["Content-Encoding: gzip"], &gzip(b"two"));
    let made = reply("201 Created", &[], b"");
    let mut keep = Keep::new(Server::owned(vec![
        vec![ok.clone(), gz],
        vec![made, ok.clone(), ok.clone()],
    ]));
    let (answer, got) = run(&mut keep, ask(Verb::Get, &at), 100);
    assert_eq!((answer.unwrap().reused, got), (false, b"one".to_vec()));
    assert_eq!(keep.held(), 1);
    let (answer, got) = run(&mut keep, ask(Verb::Get, &at), 100);
    assert_eq!((answer.unwrap().reused, got), (true, b"two".to_vec()));
    // A POST takes a new connection, and leaves it kept for the next GET.
    let request = Request {
        body: Body::Bytes(b"x=1"),
        ..ask(Verb::Post, &at)
    };
    let answer = run(&mut keep, request, 100).0.unwrap();
    assert_eq!((answer.head.status, answer.reused), (201, false));
    let (answer, _) = run(&mut keep, ask(Verb::Get, &at), 100);
    assert!(answer.unwrap().reused);
    let sent = keep.inner_mut().sent();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].matches("Connection: keep-alive").count(), 2);
    assert!(sent[1].starts_with("POST / HTTP/1.1\r\n"));
    assert!(sent[1].ends_with("\r\n\r\nx=1GET / HTTP/1.1\r\nHost: a.test\r\nUser-Agent: kai\r\nAccept-Encoding: gzip, br\r\nConnection: keep-alive\r\n\r\n"));
    // Closing answers are not kept.
    for wire in [
        reply("200 OK", &["Connection: close"], b"x"),
        b"HTTP/1.1 200 OK\r\n\r\nuntil close".to_vec(),
    ] {
        let mut keep = Keep::new(Server::owned(vec![vec![wire]]));
        assert!(run(&mut keep, ask(Verb::Get, &at), 100).0.is_ok());
        assert_eq!(keep.held(), 0);
    }
}

/// A kept connection the server closed meanwhile: a GET is made again on a
/// new one; a POST never takes it, and a POST cut before its answer is a cut,
/// never made again.
#[test]
fn a_stale_kept_connection_is_retried_only_for_a_safe_request() {
    let at = url("https://a.test/");
    let ok = reply("200 OK", &[], b"one");
    // The first connection answers once, then ends.
    let mut keep = Keep::new(Server::owned(vec![vec![ok.clone()], vec![ok.clone()]]));
    run(&mut keep, ask(Verb::Get, &at), 100).0.unwrap();
    let (answer, got) = run(&mut keep, ask(Verb::Get, &at), 100);
    let answer = answer.unwrap();
    assert_eq!((answer.reused, got), (false, b"one".to_vec()));
    assert_eq!(keep.inner_mut().sent().len(), 2);
    // A POST whose connection ends before a head: a cut, sent once.
    let mut server = Server::new(&[&[], &[b"HTTP/1.1 200 OK\r\n\r\n"]]);
    let request = Request {
        body: Body::Bytes(b"pay"),
        ..ask(Verb::Post, &at)
    };
    assert_eq!(run(&mut server, request, 100).0, Err(HttpError::Cut));
    assert_eq!(server.sent().len(), 1);
    // A DELETE with no body is not safe either.
    let mut server = Server::new(&[&[b"HTTP/1.1 200"], &[b"HTTP/1.1 200 OK\r\n\r\n"]]);
    assert_eq!(
        run(&mut server, ask(Verb::Delete, &at), 100).0,
        Err(HttpError::Cut)
    );
    assert_eq!(server.sent().len(), 1);
}

/// A redirect is the answer, never followed, with where it leads; HEAD,
/// 204 and 304 have no body; interim heads are skipped.
#[test]
fn a_redirect_is_handed_back_and_bodiless_answers_have_none() {
    let at = url("https://a.test/old");
    let mut server =
        Server::new(&[&[b"HTTP/1.1 301 Moved\r\nLocation: /new\r\nContent-Length: 4\r\n\r\ngone"]]);
    let (answer, got) = run(&mut server, ask(Verb::Get, &at), 100);
    let answer = answer.unwrap();
    assert_eq!(answer.head.status, 301);
    assert_eq!(answer.location, Some(url("https://a.test/new")));
    assert_eq!(got, b"gone");
    assert_eq!(server.sent().len(), 1);
    for (verb, wire) in [
        (
            Verb::Head,
            &b"HTTP/1.1 200 OK\r\nContent-Length: 99\r\n\r\n"[..],
        ),
        (Verb::Get, b"HTTP/1.1 204 No Content\r\n\r\n"),
        (
            Verb::Get,
            b"HTTP/1.1 304 Not Modified\r\nContent-Encoding: gzip\r\n\r\n",
        ),
    ] {
        let mut server = Server::new(&[&[wire]]);
        let answer = run(&mut server, ask(verb, &at), 100).0.unwrap();
        assert_eq!((answer.head.framing, answer.received), (Framing::Empty, 0));
    }
    let mut server = Server::new(&[&[b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 103 Early Hints\r\nLink: </a.css>\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nx"]]);
    let answer = run(&mut server, ask(Verb::Get, &at), 100).0.unwrap();
    assert_eq!((answer.head.status, answer.received), (200, 1));
}

/// A host the caller may not reach is refused before any connection, and an
/// exchange past its deadline stops.
#[test]
fn refused_hosts_and_the_deadline_stop_an_exchange() {
    let at = url("https://b.test/");
    let mut server = Server::new(&[&[b"HTTP/1.1 200 OK\r\n\r\n"]]);
    let only_a = |host: &str, _: u16| host == "a.test";
    let fetch = Fetch {
        limits: Limits::new(100, u64::MAX),
        allow: &only_a,
        clock: &zero,
    };
    let got = fetch.exchange(&mut server, ask(Verb::Get, &at), &mut |_| Ok(()));
    assert_eq!(got, Err(HttpError::Refused("b.test".into())));
    assert!(server.sent().is_empty());
    let late = || 10;
    let fetch = Fetch {
        limits: Limits::new(100, 5),
        allow: &anyone,
        clock: &late,
    };
    let got = fetch.exchange(&mut server, ask(Verb::Get, &at), &mut |_| Ok(()));
    assert_eq!(got, Err(HttpError::Timeout));
}
