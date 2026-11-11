//! The fuzz targets' bodies (plan p5-networking §1's host gates: a fuzz
//! target for every new parser of server bytes). Each `fuzz_targets/*.rs`
//! calls one of these with libFuzzer's input; `kainet/tests/fuzz_harness.rs`
//! includes this file too and runs each over hostile mutations
//! (`testkit/hostile.rs`), so a harness that cannot build, or that panics on
//! its own, fails the host tests without libFuzzer. Each must return for any
//! input, and only a bug in the code under it may panic.

#![allow(dead_code)]

use kainet::chunks::Chunks;
use kainet::http::{Connect, Fetch, HttpError, Limits, Method, Range, Stream, Url, head};
use kainet::testca::{Ca, Days, Key, KeyKind};
use kainet::tls::{Progress, Tls};
use rustls::client::UnbufferedClientConnection;
use rustls::crypto::{CryptoProvider, GetRandomFailed, SecureRandom};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::UnbufferedServerConnection;
use rustls::time_provider::TimeProvider;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

const NAME: &str = "fuzz.kai.test";
const VALID: Days = ((2025, 1, 1), (2045, 1, 1));
/// The longest body a fetch passes on here.
const MAX_BODY: u64 = 64 * 1024;

#[derive(Debug)]
struct HostRandom;

impl SecureRandom for HostRandom {
    fn fill(&self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        getrandom::fill(buf).map_err(|_| GetRandomFailed)
    }
}

static RANDOM: HostRandom = HostRandom;

#[derive(Debug)]
struct At;

impl TimeProvider for At {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(Duration::from_secs(
            1_893_456_000,
        )))
    }
}

/// What every input shares: the provider and both configurations.
struct Setup {
    client: Arc<ClientConfig>,
    server: Arc<ServerConfig>,
}

fn setup() -> &'static Setup {
    static SETUP: OnceLock<Setup> = OnceLock::new();
    SETUP.get_or_init(|| {
        let provider: Arc<CryptoProvider> = Arc::new(kainet::provider::provider(&RANDOM));
        let ca = Ca::new(
            "fuzz CA",
            Key::new(KeyKind::P256, [0x31; 32]).expect("a key"),
            VALID,
        )
        .expect("a CA");
        let key = Key::new(KeyKind::P256, [0x32; 32]).expect("a key");
        let leaf = ca.issue(&[NAME], &key, VALID).expect("a certificate");
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from(ca.der().to_vec()))
            .expect("a root");
        let client = ClientConfig::builder_with_details(provider.clone(), Arc::new(At))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("TLS 1.3")
            .with_root_certificates(roots)
            .with_no_client_auth();
        let server = ServerConfig::builder_with_details(provider, Arc::new(At))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("TLS 1.3")
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(leaf)],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.pkcs8().expect("PKCS#8"))),
            )
            .expect("a server");
        Setup {
            client: Arc::new(client),
            server: Arc::new(server),
        }
    })
}

fn client() -> Tls<UnbufferedClientConnection> {
    let name = ServerName::try_from(NAME).expect("a name");
    Tls::new(UnbufferedClientConnection::new(setup().client.clone(), name).expect("a client"))
}

/// Feeds `data` to `tls` as the peer's bytes, running it after each piece,
/// until it fails, closes or has taken them all.
fn feed<C: kainet::tls::Side>(tls: &mut Tls<C>, data: &[u8]) {
    let mut at = 0;
    while at < data.len() {
        let n = tls.feed(&data[at..]);
        at += n;
        match tls.advance(None) {
            Err(_) | Ok(Progress::Closed) => return,
            Ok(_) => {}
        }
        let read = tls.plain().len();
        tls.consume(read);
        if n == 0 {
            return;
        }
    }
}

/// The server's side of a handshake, from `data`: the record layer and
/// every handshake message's parser, before any key is agreed.
pub fn tls_client(data: &[u8]) {
    let mut c = client();
    let _ = c.advance(None);
    c.take_outgoing();
    feed(&mut c, data);
}

/// `data` as records after a whole handshake: decryption, the inner content
/// type, alerts and post-handshake messages.
pub fn tls_records(data: &[u8]) {
    let mut c = client();
    let mut s =
        Tls::new(UnbufferedServerConnection::new(setup().server.clone()).expect("a server"));
    for _ in 0..16 {
        let cp = c.advance(None);
        let out = c.take_outgoing();
        let mut at = 0;
        while at < out.len() {
            at += s.feed(&out[at..]);
        }
        let sp = s.advance(None);
        let back = s.take_outgoing();
        let mut at = 0;
        while at < back.len() {
            at += c.feed(&back[at..]);
        }
        if cp.is_err() || sp.is_err() {
            return;
        }
        if out.is_empty() && back.is_empty() {
            break;
        }
    }
    feed(&mut c, data);
}

/// A server's whole answer, `data`, to one GET: the response head and its
/// body, by length, by chunks or until the end.
pub fn http_response(data: &[u8]) {
    struct One<'a>(&'a [u8], usize);
    impl Stream for One<'_> {
        fn send(&mut self, _: &[u8]) -> Result<(), HttpError> {
            Ok(())
        }
        fn recv(&mut self, buf: &mut [u8]) -> Result<usize, HttpError> {
            // A few bytes at a time, so every parser sees split input.
            let n = buf.len().min(7).min(self.0.len() - self.1);
            buf[..n].copy_from_slice(&self.0[self.1..self.1 + n]);
            self.1 += n;
            Ok(n)
        }
    }
    struct Once<'a>(Option<&'a [u8]>);
    impl<'a> Connect for Once<'a> {
        type Stream = One<'a>;
        fn connect(&mut self, _: &Url) -> Result<One<'a>, HttpError> {
            self.0
                .take()
                .map(|d| One(d, 0))
                .ok_or(HttpError::Connect("once"))
        }
    }
    let anyone = |_: &str, _: u16| true;
    let zero = || 0;
    let fetch = Fetch {
        limits: Limits::new(MAX_BODY, u64::MAX),
        allow: &anyone,
        clock: &zero,
    };
    let url = Url::parse("https://fuzz.kai.test/blob").expect("a URL");
    let range = Range {
        first: 0,
        last: None,
    };
    let mut taken = 0u64;
    let _ = fetch.run(
        &mut Once(Some(data)),
        Method::Get,
        &url,
        Some(range),
        &mut |bytes| {
            taken += bytes.len() as u64;
            assert!(taken <= MAX_BODY, "past the body limit");
            Ok(())
        },
    );
    for method in [Method::Get, Method::Head] {
        let _ = head::parse(data, method);
    }
}

/// A `kai-chunks 1` list; one that parses renders back to itself.
pub fn kai_chunks(data: &[u8]) {
    if let Ok(list) = Chunks::parse(data) {
        assert_eq!(Chunks::parse(list.render().as_bytes()), Ok(list));
    }
}

/// A URL and a `Location:` from it; one that parses prints back to itself.
pub fn url(data: &[u8]) {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    if let Ok(url) = Url::parse(text) {
        let again = format!("https://{}{}", url.authority(), url.path);
        assert_eq!(Url::parse(&again), Ok(url.clone()));
        let _ = url.join(text);
    }
}

/// `fetch`'s messages (`kainet::wire`): a request line and an answer, as the
/// caller and `fetch` read each other's. One that reads, written again, fits
/// its buffer and reads back the same; an answer too long to send becomes
/// `wire::TOO_LONG`.
pub fn fetch_wire(data: &[u8]) {
    use kainet::wire;
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    if let Ok(ask) = wire::Ask::parse(text) {
        let again = ask.text();
        assert!(again.len() <= wire::MAX_ASK);
        assert_eq!(wire::Ask::parse(&again), Ok(ask));
    }
    if let Ok(result) = wire::parse_answer(text) {
        let again = wire::answer(&result);
        assert!(again.len() <= wire::MAX_ANSWER);
        let back = wire::parse_answer(&again).expect("an answer reads back");
        if again != wire::TOO_LONG {
            assert_eq!(back, result);
        }
    }
}
