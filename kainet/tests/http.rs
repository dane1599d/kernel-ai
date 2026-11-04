//! kainet's HTTP/1.1 client against scripted servers (each response given a
//! byte at a time as well as whole), then once over TLS on host TCP.

use kainet::http::{
    Connect, Dial, Fetch, Fetched, Framing, HttpError, Limits, Method, Range, Stream, TlsConnect,
    Url,
};
use kainet::testca::{Ca, Key, KeyKind};
use kainet::tls::{Session, Transport};
use rustls::crypto::{CryptoProvider, GetRandomFailed, SecureRandom};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, UnixTime};
use rustls::server::UnbufferedServerConnection;
use rustls::time_provider::TimeProvider;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// What each host was sent, in order.
type Log = Rc<RefCell<Vec<(String, Vec<u8>)>>>;

/// A server's answer, delivered `step` bytes at a time; `Ok(0)` at its end
/// if `clean`, else a cut.
struct Scripted {
    reply: Vec<u8>,
    at: usize,
    step: usize,
    clean: bool,
    sent: Rc<RefCell<Vec<u8>>>,
}

impl Stream for Scripted {
    fn send(&mut self, bytes: &[u8]) -> Result<(), HttpError> {
        self.sent.borrow_mut().extend_from_slice(bytes);
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, HttpError> {
        if self.at == self.reply.len() {
            return match self.clean {
                true => Ok(0),
                false => Err(HttpError::Truncated),
            };
        }
        let n = buf.len().min(self.step).min(self.reply.len() - self.at);
        buf[..n].copy_from_slice(&self.reply[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// Hosts and their one reply each; what each was sent.
struct Server {
    replies: BTreeMap<String, Vec<u8>>,
    step: usize,
    clean: bool,
    sent: Log,
}

impl Server {
    fn new(replies: &[(&str, &[u8])]) -> Self {
        Self {
            replies: replies
                .iter()
                .map(|(h, r)| (h.to_string(), r.to_vec()))
                .collect(),
            step: usize::MAX,
            clean: true,
            sent: Rc::default(),
        }
    }

    fn requests(&self) -> Vec<(String, String)> {
        self.sent
            .borrow()
            .iter()
            .map(|(h, r)| (h.clone(), String::from_utf8(r.clone()).unwrap()))
            .collect()
    }
}

struct Pending {
    host: String,
    stream: Scripted,
    log: Log,
}

impl Stream for Pending {
    fn send(&mut self, bytes: &[u8]) -> Result<(), HttpError> {
        self.log
            .borrow_mut()
            .push((self.host.clone(), bytes.to_vec()));
        self.stream.send(bytes)
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, HttpError> {
        self.stream.recv(buf)
    }
}

impl Connect for Server {
    type Stream = Pending;

    fn connect(&mut self, url: &Url) -> Result<Pending, HttpError> {
        let reply = self
            .replies
            .get(&url.host)
            .cloned()
            .ok_or(HttpError::Connect("no such host"))?;
        Ok(Pending {
            host: url.host.clone(),
            stream: Scripted {
                reply,
                at: 0,
                step: self.step,
                clean: self.clean,
                sent: Rc::default(),
            },
            log: Rc::clone(&self.sent),
        })
    }
}

const MAX: u64 = 1 << 20;

fn anyone(_: &str, _: u16) -> bool {
    true
}

fn zero() -> u64 {
    0
}

/// `method` on `url` through `server`: what came, and the body.
fn fetch_with(
    server: &mut Server,
    fetch: &Fetch,
    method: Method,
    url: &str,
    range: Option<Range>,
) -> (Result<Fetched, HttpError>, Vec<u8>) {
    let mut body = Vec::new();
    let got = fetch.run(server, method, &Url::parse(url).unwrap(), range, &mut |d| {
        body.extend_from_slice(d);
        Ok(())
    });
    (got, body)
}

fn get(server: &mut Server, url: &str) -> (Result<Fetched, HttpError>, Vec<u8>) {
    let fetch = Fetch {
        limits: Limits::new(MAX, u64::MAX),
        allow: &anyone,
        clock: &zero,
    };
    fetch_with(server, &fetch, Method::Get, url, None)
}

/// Every step size a reply is delivered in.
const STEPS: [usize; 4] = [1, 3, 100, usize::MAX];

#[test]
fn a_body_by_length_comes_whole_in_any_pieces() {
    let reply = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nServer: x\r\n\r\nhello";
    for step in STEPS {
        let mut server = Server::new(&[("a.test", reply)]);
        server.step = step;
        let (got, body) = get(&mut server, "https://a.test/f");
        let got = got.unwrap();
        assert_eq!(
            (got.head.status, got.received, body.as_slice()),
            (200, 5, &b"hello"[..])
        );
        assert_eq!(got.head.framing, Framing::Length(5));
    }
}

#[test]
fn the_request_asks_for_no_compression_and_one_exchange() {
    let mut server = Server::new(&[("a.test", b"HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\nContent-Range: bytes 10-12/100\r\n\r\nabc")]);
    let fetch = Fetch {
        limits: Limits::new(MAX, u64::MAX),
        allow: &anyone,
        clock: &zero,
    };
    let range = Range {
        first: 10,
        last: Some(12),
    };
    let (got, body) = fetch_with(
        &mut server,
        &fetch,
        Method::Get,
        "https://a.test/x?y",
        Some(range),
    );
    assert_eq!(
        (got.unwrap().head.status, body.as_slice()),
        (206, &b"abc"[..])
    );
    assert_eq!(
        server.requests(),
        [(
            "a.test".to_string(),
            "GET /x?y HTTP/1.1\r\nHost: a.test\r\nUser-Agent: kai\r\nAccept-Encoding: identity\r\nConnection: close\r\nRange: bytes=10-12\r\n\r\n".to_string()
        )]
    );
}

#[test]
fn a_chunked_body_comes_whole_in_any_pieces() {
    let reply = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nwiki\r\n5;x=y\r\npedia\r\n0\r\nT: 1\r\n\r\n";
    for step in STEPS {
        let mut server = Server::new(&[("a.test", reply)]);
        server.step = step;
        let (got, body) = get(&mut server, "https://a.test/");
        assert_eq!(got.unwrap().head.framing, Framing::Chunked);
        assert_eq!(body, b"wikipedia");
    }
}

#[test]
fn a_body_until_close_needs_a_clean_end() {
    let reply = b"HTTP/1.1 200 OK\r\n\r\nall of it";
    let mut server = Server::new(&[("a.test", reply)]);
    let (got, body) = get(&mut server, "https://a.test/");
    assert_eq!(
        (got.unwrap().head.framing, body.as_slice()),
        (Framing::Close, &b"all of it"[..])
    );
    let mut cut = Server::new(&[("a.test", reply)]);
    cut.clean = false;
    assert_eq!(
        get(&mut cut, "https://a.test/").0,
        Err(HttpError::Truncated)
    );
}

#[test]
fn a_body_short_of_its_length_or_its_last_chunk_is_truncated() {
    for reply in [
        &b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort"[..],
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n",
    ] {
        let mut server = Server::new(&[("a.test", reply)]);
        assert_eq!(
            get(&mut server, "https://a.test/").0,
            Err(HttpError::Truncated),
            "{reply:?}"
        );
    }
}

#[test]
fn hostile_heads_are_refused() {
    for reply in [
        &b"HTTP/1.0 200 OK\r\nContent-Length: 0\r\n\r\n"[..],
        b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab",
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 0\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Length: 99999999999999999999\r\n\r\n",
        b"HTTP/1.1 2000 OK\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nBad Header\r\n\r\n",
        b"NOT HTTP\r\n\r\n",
        b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 5-2/9\r\nContent-Length: 0\r\n\r\n",
        b"HTTP/1.1 302 Found\r\nContent-Length: 0\r\n\r\n",
        b"HTTP/1.1 302 Found\r\nLocation: /a\r\nLocation: /b\r\n\r\n",
    ] {
        let mut server = Server::new(&[("a.test", reply)]);
        let got = get(&mut server, "https://a.test/").0;
        assert!(
            matches!(got, Err(HttpError::Head(_))),
            "{:?}: {got:?}",
            String::from_utf8_lossy(reply)
        );
    }
    let many = format!("HTTP/1.1 200 OK\r\n{}\r\n", "X-A: b\r\n".repeat(65));
    let mut server = Server::new(&[("a.test", many.as_bytes())]);
    assert!(matches!(
        get(&mut server, "https://a.test/").0,
        Err(HttpError::Head(_))
    ));
}

#[test]
fn a_head_or_body_over_its_limit_is_too_large() {
    let big = format!("HTTP/1.1 200 OK\r\nX-Pad: {}\r\n\r\n", "a".repeat(20_000));
    let mut server = Server::new(&[("a.test", big.as_bytes())]);
    assert_eq!(
        get(&mut server, "https://a.test/").0,
        Err(HttpError::TooLarge)
    );
    let fetch = Fetch {
        limits: Limits::new(4, u64::MAX),
        allow: &anyone,
        clock: &zero,
    };
    for reply in [
        &b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello"[..],
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        b"HTTP/1.1 200 OK\r\n\r\nhello",
    ] {
        let mut server = Server::new(&[("a.test", reply)]);
        let got = fetch_with(&mut server, &fetch, Method::Get, "https://a.test/", None).0;
        assert_eq!(got, Err(HttpError::TooLarge), "{reply:?}");
    }
}

#[test]
fn redirects_are_followed_only_to_allowed_hosts_keeping_the_range() {
    let mut server = Server::new(&[
        ("releases.test", b"HTTP/1.1 302 Found\r\nLocation: https://assets.test/blob\r\nContent-Length: 0\r\n\r\n"),
        ("assets.test", b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 4-7/10\r\nContent-Length: 4\r\n\r\nabcd"),
    ]);
    let allowed =
        |host: &str, port: u16| port == 443 && (host == "releases.test" || host == "assets.test");
    let fetch = Fetch {
        limits: Limits::new(MAX, u64::MAX),
        allow: &allowed,
        clock: &zero,
    };
    let range = Some(Range {
        first: 4,
        last: None,
    });
    let (got, body) = fetch_with(
        &mut server,
        &fetch,
        Method::Get,
        "https://releases.test/d",
        range,
    );
    let got = got.unwrap();
    assert_eq!(
        (got.url.host.as_str(), body.as_slice()),
        ("assets.test", &b"abcd"[..])
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].1.contains("Range: bytes=4-\r\n"),
        "{requests:?}"
    );
    let only_releases = |host: &str, _: u16| host == "releases.test";
    let refused = Fetch {
        allow: &only_releases,
        ..fetch
    };
    let got = fetch_with(
        &mut server,
        &refused,
        Method::Get,
        "https://releases.test/d",
        None,
    )
    .0;
    assert_eq!(got, Err(HttpError::Refused("assets.test".into())));
}

#[test]
fn more_than_five_redirects_stop() {
    let reply = b"HTTP/1.1 301 Moved\r\nLocation: /again\r\nContent-Length: 0\r\n\r\n";
    let mut server = Server::new(&[("loop.test", reply)]);
    assert_eq!(
        get(&mut server, "https://loop.test/").0,
        Err(HttpError::TooManyRedirects)
    );
    assert_eq!(server.requests().len(), 6);
}

#[test]
fn another_status_comes_back_without_its_body() {
    let reply =
        b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 30\r\nContent-Length: 4\r\n\r\nslow";
    let mut server = Server::new(&[("a.test", reply)]);
    let (got, body) = get(&mut server, "https://a.test/");
    let got = got.unwrap();
    assert_eq!(
        (got.head.status, got.head.retry_after, body.len()),
        (429, Some(30), 0)
    );
}

#[test]
fn head_has_no_body_and_a_206_must_be_for_the_range_asked() {
    let mut server = Server::new(&[("a.test", b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n")]);
    let fetch = Fetch {
        limits: Limits::new(MAX, u64::MAX),
        allow: &anyone,
        clock: &zero,
    };
    let got = fetch_with(&mut server, &fetch, Method::Head, "https://a.test/", None).0;
    assert_eq!(got.unwrap().head.framing, Framing::Empty);
    let wrong = b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-3/10\r\nContent-Length: 4\r\n\r\nabcd";
    let mut server = Server::new(&[("a.test", wrong)]);
    let range = Some(Range {
        first: 4,
        last: Some(7),
    });
    let got = fetch_with(&mut server, &fetch, Method::Get, "https://a.test/", range).0;
    assert!(matches!(got, Err(HttpError::Head(_))), "{got:?}");
}

/// A `Content-Type` is kept when it is at most 128 printable bytes, and only
/// then (C's review of #671).
#[test]
fn a_content_type_is_kept_only_when_short_and_printable() {
    let long = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: 0\r\n\r\n",
        "a".repeat(129)
    );
    for (reply, kept) in [
        (&b"HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: 0\r\n\r\n"[..], Some("application/json; charset=utf-8")),
        (b"HTTP/1.1 200 OK\r\nContent-Type: text/\tplain\r\nContent-Length: 0\r\n\r\n", None),
        (long.as_bytes(), None),
        (b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n", None),
    ] {
        let mut server = Server::new(&[("a.test", reply)]);
        let got = get(&mut server, "https://a.test/").0.unwrap();
        assert_eq!(got.head.content_type.as_deref(), kept);
    }
}

/// A server that ignores a range and sends the whole target must not have it
/// passed on as the range: a resumed download would take the target's first
/// bytes for the range's (C's review of #658).
#[test]
fn a_200_to_a_range_passes_nothing_on() {
    let whole = b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n0123456789";
    let fetch = Fetch {
        limits: Limits::new(MAX, u64::MAX),
        allow: &anyone,
        clock: &zero,
    };
    for range in [
        Range {
            first: 4,
            last: None,
        },
        Range {
            first: 4,
            last: Some(7),
        },
        Range {
            first: 0,
            last: Some(3),
        },
    ] {
        let mut server = Server::new(&[("a.test", whole)]);
        let (got, body) = fetch_with(
            &mut server,
            &fetch,
            Method::Get,
            "https://a.test/",
            Some(range),
        );
        assert_eq!(
            (got, body.len()),
            (Err(HttpError::RangeIgnored), 0),
            "{range:?}"
        );
    }
    // A range from byte 0 to the end is the whole target, so a 200 is it.
    let mut server = Server::new(&[("a.test", whole)]);
    let from_start = Some(Range {
        first: 0,
        last: None,
    });
    let (got, body) = fetch_with(
        &mut server,
        &fetch,
        Method::Get,
        "https://a.test/",
        from_start,
    );
    assert_eq!(
        (got.unwrap().received, body.as_slice()),
        (10, &b"0123456789"[..])
    );
}

#[test]
fn interim_heads_are_skipped_and_a_protocol_switch_refused() {
    let early = b"HTTP/1.1 103 Early Hints\r\nLink: </a>\r\n\r\nHTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
    for step in STEPS {
        let mut server = Server::new(&[("a.test", early)]);
        server.step = step;
        let (got, body) = get(&mut server, "https://a.test/");
        assert_eq!(
            (got.unwrap().head.status, body.as_slice()),
            (200, &b"ok"[..]),
            "step {step}"
        );
    }
    let switch = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: h2c\r\n\r\n";
    let mut server = Server::new(&[("a.test", switch)]);
    assert!(matches!(
        get(&mut server, "https://a.test/").0,
        Err(HttpError::Head(_))
    ));
    let endless = b"HTTP/1.1 100 Continue\r\n\r\n".repeat(10);
    let mut server = Server::new(&[("a.test", &endless)]);
    assert!(matches!(
        get(&mut server, "https://a.test/").0,
        Err(HttpError::Head(_))
    ));
}

#[test]
fn the_caller_is_asked_about_each_host_and_port() {
    let mut server = Server::new(&[(
        "a.test",
        b"HTTP/1.1 302 Found\r\nLocation: https://a.test:8443/x\r\nContent-Length: 0\r\n\r\n",
    )]);
    let only_443 = |_: &str, port: u16| port == 443;
    let fetch = Fetch {
        limits: Limits::new(MAX, u64::MAX),
        allow: &only_443,
        clock: &zero,
    };
    let got = fetch_with(&mut server, &fetch, Method::Get, "https://a.test/", None).0;
    assert_eq!(got, Err(HttpError::Refused("a.test".into())));
}

#[test]
fn a_fetch_past_its_deadline_stops() {
    let ticks = Cell::new(0u64);
    let clock = || {
        ticks.set(ticks.get() + 1);
        ticks.get()
    };
    let fetch = Fetch {
        limits: Limits::new(MAX, 3),
        allow: &anyone,
        clock: &clock,
    };
    let mut server = Server::new(&[(
        "a.test",
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello",
    )]);
    server.step = 1;
    let got = fetch_with(&mut server, &fetch, Method::Get, "https://a.test/", None).0;
    assert_eq!(got, Err(HttpError::Timeout));
}

#[derive(Debug)]
struct HostRandom;

impl SecureRandom for HostRandom {
    fn fill(&self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        getrandom::fill(buf).map_err(|_| GetRandomFailed)
    }
}

static RANDOM: HostRandom = HostRandom;

fn ours() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(kainet::provider::provider(&RANDOM)))
        .clone()
}

#[derive(Debug)]
struct At;

impl TimeProvider for At {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(Duration::from_secs(
            1_780_000_000,
        )))
    }
}

struct Tcp(std::net::TcpStream);

impl Transport for Tcp {
    fn send(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        std::io::Write::write_all(&mut self.0, bytes).map_err(|_| "send failed")
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, &'static str> {
        std::io::Read::read(&mut self.0, buf).map_err(|_| "recv failed")
    }
}

struct Local(u16);

impl Dial for Local {
    type Transport = Tcp;

    fn dial(&mut self, _: &str, _: u16) -> Result<Tcp, HttpError> {
        std::net::TcpStream::connect(("127.0.0.1", self.0))
            .map(Tcp)
            .map_err(|_| HttpError::Connect("refused"))
    }
}

#[test]
fn a_fetch_over_tls_reads_a_chunked_body() {
    let valid = ((2025, 1, 1), (2030, 1, 1));
    let ca = Ca::new(
        "http test CA",
        Key::new(KeyKind::P256, [3; 32]).unwrap(),
        valid,
    )
    .unwrap();
    let key = Key::new(KeyKind::P256, [4; 32]).unwrap();
    let leaf = ca.issue(&["files.test"], &key, valid).unwrap();
    let server_config = ServerConfig::builder_with_details(ours(), Arc::new(At))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(leaf)],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.pkcs8().unwrap())),
        )
        .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let served = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let conn = UnbufferedServerConnection::new(Arc::new(server_config)).unwrap();
        let mut session = Session::new(conn, Tcp(stream));
        session.handshake().unwrap();
        let mut head = [0u8; 512];
        let n = session.read(&mut head).unwrap();
        assert!(head[..n].starts_with(b"GET /blob HTTP/1.1\r\nHost: files.test\r\n"));
        let body = "a".repeat(40_000);
        let reply = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
            body.len()
        );
        session.write_all(reply.as_bytes()).unwrap();
        session.close().unwrap();
    });
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(ca.der().to_vec())).unwrap();
    let config = ClientConfig::builder_with_details(ours(), Arc::new(At))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let mut connect = TlsConnect {
        config: Arc::new(config),
        dial: Local(port),
    };
    let fetch = Fetch {
        limits: Limits::new(MAX, u64::MAX),
        allow: &anyone,
        clock: &zero,
    };
    let mut body = Vec::new();
    let got = fetch
        .run(
            &mut connect,
            Method::Get,
            &Url::parse("https://files.test/blob").unwrap(),
            None,
            &mut |d| {
                body.extend_from_slice(d);
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(
        (got.head.status, got.received, body.len()),
        (200, 40_000, 40_000)
    );
    served.join().unwrap();
}
