//! Our provider against itself in memory, and over a host TCP socket: every
//! suite and group, CA keys of each kind, data both ways, and what must be
//! refused (a wrong name, an expired certificate, an unknown CA, a tampered
//! record, a cut connection).

use kainet::provider;
use kainet::testca::{Ca, Days, Key, KeyKind};
use kainet::tls::{Progress, Session, Tls, TlsError, Transport};
use rustls::client::UnbufferedClientConnection;
use rustls::crypto::{CryptoProvider, GetRandomFailed, SecureRandom};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::UnbufferedServerConnection;
use rustls::time_provider::TimeProvider;
use rustls::{CertificateError, ClientConfig, Error, NamedGroup, RootCertStore, ServerConfig};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// 2026-05-28, inside [`VALID`].
const NOW: u64 = 1_780_000_000;
const VALID: Days = ((2025, 1, 1), (2030, 1, 1));
const EXPIRED: Days = ((2020, 1, 1), (2021, 1, 1));
const NAME: &str = "mock.test";

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
        .get_or_init(|| Arc::new(provider::provider(&RANDOM)))
        .clone()
}

#[derive(Debug)]
struct At(u64);

impl TimeProvider for At {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(Duration::from_secs(self.0)))
    }
}

fn seed(n: u8) -> [u8; 32] {
    let mut seed = [0x11u8; 32];
    seed[0] = n;
    seed
}

/// A CA of `kind` and a P-256 server certificate from it for `names`.
struct Pki {
    ca: Ca,
    leaf: Vec<u8>,
    key: Vec<u8>,
}

fn pki(kind: KeyKind, names: &[&str], valid: Days) -> Pki {
    let ca = Ca::new("kainet test CA", Key::new(kind, seed(1)).unwrap(), VALID).unwrap();
    let key = Key::new(KeyKind::P256, seed(2)).unwrap();
    let leaf = ca.issue(names, &key, valid).unwrap();
    Pki {
        ca,
        leaf,
        key: key.pkcs8().unwrap(),
    }
}

fn client_with(
    root: &[u8],
    name: &str,
    provider: CryptoProvider,
) -> Tls<UnbufferedClientConnection> {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(root.to_vec())).unwrap();
    let config = ClientConfig::builder_with_details(Arc::new(provider), Arc::new(At(NOW)))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = ServerName::try_from(name.to_string()).unwrap();
    Tls::new(UnbufferedClientConnection::new(Arc::new(config), name).unwrap())
}

fn client(root: &[u8], name: &str) -> Tls<UnbufferedClientConnection> {
    client_with(root, name, (*ours()).clone())
}

fn server(pki: &Pki) -> Tls<UnbufferedServerConnection> {
    let config = ServerConfig::builder_with_details(ours(), Arc::new(At(NOW)))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(pki.leaf.clone())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pki.key.clone())),
        )
        .unwrap();
    Tls::new(UnbufferedServerConnection::new(Arc::new(config)).unwrap())
}

type Client = Tls<UnbufferedClientConnection>;
type Server = Tls<UnbufferedServerConnection>;

/// Moves `from`'s outgoing bytes to `to`, all of them.
fn carry(bytes: Vec<u8>, to: &mut impl FnMut(&[u8]) -> usize) {
    let mut at = 0;
    while at < bytes.len() {
        let taken = to(&bytes[at..]);
        assert!(taken > 0, "the peer took nothing");
        at += taken;
    }
}

/// Runs both sides until neither has anything to send: each side's last
/// progress, or its error.
fn pump(
    c: &mut Client,
    s: &mut Server,
) -> (Result<Progress, TlsError>, Result<Progress, TlsError>) {
    let mut last = (Ok(Progress::NeedInput), Ok(Progress::NeedInput));
    for _ in 0..32 {
        let cp = c.advance(None);
        let to_server = c.take_outgoing();
        carry(to_server.clone(), &mut |b| s.feed(b));
        let sp = s.advance(None);
        let to_client = s.take_outgoing();
        carry(to_client.clone(), &mut |b| c.feed(b));
        let failed = cp.is_err() || sp.is_err();
        last = (cp, sp);
        if failed || (to_server.is_empty() && to_client.is_empty()) {
            break;
        }
    }
    last
}

fn handshake(c: &mut Client, s: &mut Server) {
    let (cp, sp) = pump(c, s);
    assert_eq!(cp, Ok(Progress::Ready));
    assert_eq!(sp, Ok(Progress::Ready));
}

/// `data` written by `from`'s side and read by the other, through `pump`'s
/// carrying; what came out.
fn send_to_client(s: &mut Server, c: &mut Client, data: &[u8]) -> Vec<u8> {
    let mut got = Vec::new();
    for chunk in data.chunks(32 * 1024) {
        assert_eq!(s.advance(Some(chunk)), Ok(Progress::Wrote));
        let bytes = s.take_outgoing();
        let mut at = 0;
        while at < bytes.len() {
            at += c.feed(&bytes[at..]);
            c.advance(None).unwrap();
            got.extend_from_slice(c.plain());
            let n = c.plain().len();
            c.consume(n);
        }
    }
    got
}

fn failure(result: (Result<Progress, TlsError>, Result<Progress, TlsError>)) -> Error {
    match result.0 {
        Err(TlsError::Tls(error)) => error,
        other => panic!("the client went on: {other:?}"),
    }
}

#[test]
fn every_suite_and_group_handshakes() {
    let pki = pki(KeyKind::P256, &[NAME], VALID);
    for suite in ours().cipher_suites.clone() {
        for group in ours().kx_groups.clone() {
            let mut provider = (*ours()).clone();
            provider.cipher_suites = vec![suite];
            provider.kx_groups = vec![group];
            let mut c = client_with(pki.ca.der(), NAME, provider);
            let mut s = server(&pki);
            handshake(&mut c, &mut s);
            let conn = c.connection();
            assert_eq!(conn.negotiated_cipher_suite(), Some(suite));
            assert_eq!(
                conn.negotiated_key_exchange_group().map(|g| g.name()),
                Some(group.name())
            );
        }
    }
}

#[test]
fn x25519_is_the_first_choice() {
    let pki = pki(KeyKind::P256, &[NAME], VALID);
    let mut c = client(pki.ca.der(), NAME);
    let mut s = server(&pki);
    handshake(&mut c, &mut s);
    let group = c.connection().negotiated_key_exchange_group().unwrap();
    assert_eq!(group.name(), NamedGroup::X25519);
}

#[test]
fn a_chain_from_each_kind_of_ca_key_verifies() {
    for kind in [KeyKind::P256, KeyKind::P384, KeyKind::Ed25519] {
        let pki = pki(kind, &[NAME], VALID);
        let mut c = client(pki.ca.der(), NAME);
        let mut s = server(&pki);
        handshake(&mut c, &mut s);
    }
}

#[test]
fn data_goes_both_ways_intact() {
    let pki = pki(KeyKind::P256, &[NAME], VALID);
    let mut c = client(pki.ca.der(), NAME);
    let mut s = server(&pki);
    handshake(&mut c, &mut s);
    let data: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
    assert_eq!(send_to_client(&mut s, &mut c, &data), data);
    assert_eq!(
        c.advance(Some(b"GET / HTTP/1.1\r\n\r\n")),
        Ok(Progress::Wrote)
    );
    carry(c.take_outgoing(), &mut |b| s.feed(b));
    s.advance(None).unwrap();
    assert_eq!(s.plain(), b"GET / HTTP/1.1\r\n\r\n");
}

#[test]
fn a_certificate_for_another_name_is_refused() {
    let pki = pki(KeyKind::P256, &["other.test"], VALID);
    let mut c = client(pki.ca.der(), NAME);
    let mut s = server(&pki);
    let (client_side, server_side) = pump(&mut c, &mut s);
    let error = failure((client_side, Ok(Progress::NeedInput)));
    assert!(
        matches!(
            error,
            Error::InvalidCertificate(CertificateError::NotValidForNameContext { .. })
        ),
        "{error:?}"
    );
    // The client's alert ended the server's side too.
    assert!(server_side.is_err(), "{server_side:?}");
}

#[test]
fn an_expired_certificate_is_refused() {
    let pki = pki(KeyKind::P256, &[NAME], EXPIRED);
    let mut c = client(pki.ca.der(), NAME);
    let mut s = server(&pki);
    let error = failure(pump(&mut c, &mut s));
    assert!(
        matches!(
            error,
            Error::InvalidCertificate(CertificateError::ExpiredContext { .. })
        ),
        "{error:?}"
    );
}

#[test]
fn a_certificate_from_an_unknown_ca_is_refused() {
    let pki = pki(KeyKind::P256, &[NAME], VALID);
    let other = Ca::new(
        "another CA",
        Key::new(KeyKind::P256, seed(9)).unwrap(),
        VALID,
    )
    .unwrap();
    let mut c = client(other.der(), NAME);
    let mut s = server(&pki);
    let error = failure(pump(&mut c, &mut s));
    assert_eq!(
        error,
        Error::InvalidCertificate(CertificateError::UnknownIssuer)
    );
}

#[test]
fn a_tampered_record_is_refused() {
    let pki = pki(KeyKind::P256, &[NAME], VALID);
    let mut c = client(pki.ca.der(), NAME);
    let mut s = server(&pki);
    handshake(&mut c, &mut s);
    assert_eq!(s.advance(Some(b"the real answer")), Ok(Progress::Wrote));
    let mut bytes = s.take_outgoing();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    c.feed(&bytes);
    assert_eq!(c.advance(None), Err(TlsError::Tls(Error::DecryptError)));
}

/// A transport over a fixed script of the peer's bytes, which then ends.
struct Cut {
    peer: Vec<u8>,
    at: usize,
}

impl Transport for Cut {
    fn send(&mut self, _: &[u8]) -> Result<(), &'static str> {
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, &'static str> {
        let n = buf.len().min(self.peer.len() - self.at);
        buf[..n].copy_from_slice(&self.peer[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

#[test]
fn a_connection_cut_in_the_handshake_is_truncated() {
    let pki = pki(KeyKind::P256, &[NAME], VALID);
    let conn = client(pki.ca.der(), NAME).into_connection();
    let mut session = Session::new(
        conn,
        Cut {
            peer: Vec::new(),
            at: 0,
        },
    );
    assert_eq!(session.handshake(), Err(TlsError::Truncated));
}

/// The host socket as a transport.
struct Tcp(std::net::TcpStream);

impl Transport for Tcp {
    fn send(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        std::io::Write::write_all(&mut self.0, bytes).map_err(|_| "send failed")
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, &'static str> {
        std::io::Read::read(&mut self.0, buf).map_err(|_| "recv failed")
    }
}

/// A server on a host socket that answers "hello" with `size` bytes, then
/// sends a close_notify if `close`, or else just drops the socket.
fn serve_once(pki: &Pki, size: usize, close: bool) -> (u16, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let tls = server(pki);
    let served = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut session = Session::new(tls.into_connection(), Tcp(stream));
        session.handshake().unwrap();
        let mut request = [0u8; 64];
        let n = session.read(&mut request).unwrap();
        assert_eq!(&request[..n], b"hello");
        session.write_all(&vec![b'x'; size]).unwrap();
        if close {
            session.close().unwrap();
        }
    });
    (port, served)
}

/// Connects, says hello and reads to the end: what came, and how it ended.
fn fetch(pki: &Pki, port: u16) -> (usize, Result<(), TlsError>) {
    let stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut session = Session::new(client(pki.ca.der(), NAME).into_connection(), Tcp(stream));
    session.handshake().unwrap();
    session.write_all(b"hello").unwrap();
    let mut got = 0;
    let mut buf = [0u8; 4096];
    loop {
        match session.read(&mut buf) {
            Ok(0) => return (got, Ok(())),
            Ok(n) => got += n,
            Err(error) => return (got, Err(error)),
        }
    }
}

#[test]
fn a_session_over_tcp_reads_until_close_notify() {
    let pki = pki(KeyKind::P256, &[NAME], VALID);
    let (port, served) = serve_once(&pki, 100_000, true);
    assert_eq!(fetch(&pki, port), (100_000, Ok(())));
    served.join().unwrap();
}

#[test]
fn a_session_cut_without_close_notify_is_truncated() {
    let pki = pki(KeyKind::P256, &[NAME], VALID);
    let (port, served) = serve_once(&pki, 50_000, false);
    assert_eq!(fetch(&pki, port), (50_000, Err(TlsError::Truncated)));
    served.join().unwrap();
}
