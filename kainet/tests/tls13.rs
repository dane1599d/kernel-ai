//! Task 4's refusals and hostile input: a clock before the build, a server
//! that answers in TLS 1.2, a certificate with a signature algorithm we do
//! not take, and random or mutated records from the server, before and after
//! the handshake. None may panic; each must end the connection.

use kainet::client::{self, Wall};
use kainet::provider;
use kainet::testca::{Ca, Days, Key, KeyKind};
use kainet::tls::{Progress, Tls, TlsError};
use rustls::client::UnbufferedClientConnection;
use rustls::crypto::{CryptoProvider, GetRandomFailed, SecureRandom};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::server::UnbufferedServerConnection;
use rustls::{CertificateError, Error, RootCertStore, ServerConfig};
use std::sync::{Arc, OnceLock};

const VALID: Days = ((2025, 1, 1), (2045, 1, 1));
const NAME: &str = "mock.test";
/// 2030-01-01.
const NOW_S: u64 = 1_893_456_000;

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

fn at_2030() -> Option<u64> {
    Some(NOW_S)
}

fn before_the_build() -> Option<u64> {
    Some(client::FLOOR_S - 1)
}

fn no_clock() -> Option<u64> {
    None
}

struct Pki {
    ca: Vec<u8>,
    leaf: Vec<u8>,
    key: Vec<u8>,
}

fn pki() -> Pki {
    let ca = Ca::new("tls13 CA", Key::new(KeyKind::P256, [5; 32]).unwrap(), VALID).unwrap();
    let key = Key::new(KeyKind::P256, [6; 32]).unwrap();
    let leaf = ca.issue(&[NAME], &key, VALID).unwrap();
    Pki {
        ca: ca.der().to_vec(),
        leaf,
        key: key.pkcs8().unwrap(),
    }
}

fn client(root: &[u8], now_s: fn() -> Option<u64>) -> Tls<UnbufferedClientConnection> {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(root.to_vec())).unwrap();
    let wall = Wall {
        now_s,
        floor_s: client::FLOOR_S,
    };
    let config = client::config(ours(), roots, Arc::new(wall)).unwrap();
    let name = ServerName::try_from(NAME).unwrap();
    Tls::new(UnbufferedClientConnection::new(config, name).unwrap())
}

fn server(leaf: &[u8], key: &[u8]) -> Tls<UnbufferedServerConnection> {
    let config = ServerConfig::builder_with_details(
        ours(),
        Arc::new(Wall {
            now_s: at_2030,
            floor_s: 0,
        }),
    )
    .with_protocol_versions(&[&rustls::version::TLS13])
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![CertificateDer::from(leaf.to_vec())],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.to_vec())),
    )
    .unwrap();
    Tls::new(UnbufferedServerConnection::new(Arc::new(config)).unwrap())
}

fn feed_all<C: kainet::tls::Side>(to: &mut Tls<C>, bytes: &[u8]) {
    let mut at = 0;
    while at < bytes.len() {
        let n = to.feed(&bytes[at..]);
        assert!(n > 0);
        at += n;
    }
}

/// Runs both sides until one fails or neither sends: the client's last word.
fn pump(
    c: &mut Tls<UnbufferedClientConnection>,
    s: &mut Tls<UnbufferedServerConnection>,
) -> Result<Progress, TlsError> {
    let mut last = Ok(Progress::NeedInput);
    for _ in 0..16 {
        last = c.advance(None);
        let out = c.take_outgoing();
        feed_all(s, &out);
        let sp = s.advance(None);
        let back = s.take_outgoing();
        feed_all(c, &back);
        if last.is_err() || sp.is_err() || (out.is_empty() && back.is_empty()) {
            break;
        }
    }
    last
}

#[test]
fn a_clock_before_the_build_or_none_refuses_every_certificate() {
    let pki = pki();
    for now in [before_the_build as fn() -> Option<u64>, no_clock] {
        let mut c = client(&pki.ca, now);
        let mut s = server(&pki.leaf, &pki.key);
        assert_eq!(
            pump(&mut c, &mut s),
            Err(TlsError::Tls(Error::FailedToGetCurrentTime))
        );
    }
    let mut c = client(&pki.ca, at_2030);
    let mut s = server(&pki.leaf, &pki.key);
    assert_eq!(pump(&mut c, &mut s), Ok(Progress::Ready));
}

/// A TLS 1.2 ServerHello: no supported_versions, ECDHE-ECDSA-AES128-GCM.
fn tls12_server_hello() -> Vec<u8> {
    let mut body = vec![0x03, 0x03];
    body.extend_from_slice(&[0x42; 32]);
    body.push(0);
    body.extend_from_slice(&[0xc0, 0x2b, 0x00, 0x00, 0x00]);
    let mut handshake = vec![0x02, 0, 0, body.len() as u8];
    handshake.extend_from_slice(&body);
    let mut record = vec![0x16, 0x03, 0x03, 0, handshake.len() as u8];
    record.extend_from_slice(&handshake);
    record
}

#[test]
fn a_server_that_answers_in_tls12_is_refused() {
    let pki = pki();
    let mut c = client(&pki.ca, at_2030);
    assert_eq!(c.advance(None), Ok(Progress::NeedInput));
    let hello = c.take_outgoing();
    assert_eq!(hello[0], 0x16, "a ClientHello went out");
    feed_all(&mut c, &tls12_server_hello());
    let got = c.advance(None);
    assert!(
        matches!(
            got,
            Err(TlsError::Tls(
                Error::PeerIncompatible(_) | Error::PeerMisbehaved(_)
            ))
        ),
        "{got:?}"
    );
    // It said why, with an alert.
    assert_eq!(c.take_outgoing().first(), Some(&0x15));
}

/// `der` with every `from` replaced by `to`, of the same length.
fn replaced(der: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    assert_eq!(from.len(), to.len());
    let mut out = der.to_vec();
    let mut i = 0;
    let mut found = 0;
    while i + from.len() <= out.len() {
        if &out[i..i + from.len()] == from {
            out[i..i + from.len()].copy_from_slice(to);
            found += 1;
            i += from.len();
        } else {
            i += 1;
        }
    }
    assert!(found >= 2, "both signature fields");
    out
}

#[test]
fn a_certificate_signed_with_an_algorithm_we_do_not_take_is_refused() {
    let pki = pki();
    // ecdsa-with-SHA256 becomes ecdsa-with-SHA224, which no provider of ours
    // checks, in the certificate's signature fields alike.
    let sha256 = [0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
    let sha224 = [0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x01];
    let weak = replaced(&pki.leaf, &sha256, &sha224);
    let mut c = client(&pki.ca, at_2030);
    let mut s = server(&weak, &pki.key);
    let got = pump(&mut c, &mut s);
    assert!(
        matches!(
            got,
            Err(TlsError::Tls(Error::InvalidCertificate(
                CertificateError::UnsupportedSignatureAlgorithmContext { .. }
                    | CertificateError::UnsupportedSignatureAlgorithmForPublicKeyContext { .. }
                    | CertificateError::BadSignature
            )))
        ),
        "{got:?}"
    );
}

/// A small, fixed random source for the hostile inputs.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

/// A record header of a random type and version, and a random body.
fn hostile_record(rng: &mut XorShift) -> Vec<u8> {
    let len = (rng.next() % 600) as usize;
    let kind = [0x14, 0x15, 0x16, 0x17, 0x18, 0x00, 0xff][(rng.next() % 7) as usize];
    let version = [0x0301, 0x0303, 0x0304, 0x0000][(rng.next() % 4) as usize] as u16;
    let mut record = vec![kind];
    record.extend_from_slice(&version.to_be_bytes());
    let claimed = if rng.next().is_multiple_of(4) {
        rng.next() as u16
    } else {
        len as u16
    };
    record.extend_from_slice(&claimed.to_be_bytes());
    record.extend(rng.bytes(len));
    record
}

#[test]
fn random_records_before_the_handshake_end_it_without_a_panic() {
    let pki = pki();
    let mut rng = XorShift(0x1234_5678_9abc_def1);
    for _ in 0..500 {
        let mut c = client(&pki.ca, at_2030);
        c.advance(None).unwrap();
        c.take_outgoing();
        let record = hostile_record(&mut rng);
        let taken = c.feed(&record);
        match c.advance(None) {
            Err(_) | Ok(Progress::NeedInput) => {}
            other => panic!("a random record was taken: {other:?} after {taken} bytes"),
        }
    }
}

#[test]
fn mutated_records_after_the_handshake_are_refused_without_a_panic() {
    let pki = pki();
    let mut rng = XorShift(0x0fed_cba9_8765_4321);
    for round in 0..60u32 {
        let mut c = client(&pki.ca, at_2030);
        let mut s = server(&pki.leaf, &pki.key);
        assert_eq!(pump(&mut c, &mut s), Ok(Progress::Ready));
        assert_eq!(
            s.advance(Some(b"a genuine record of data")),
            Ok(Progress::Wrote)
        );
        let mut record = s.take_outgoing();
        let genuine = record.clone();
        let at = 5 + (rng.next() as usize) % (record.len() - 5);
        record[at] ^= 1 << (rng.next() % 8);
        // Cut after the flipped byte, never before it: cut at it, the random
        // tail could stand in for the genuine record's last bytes. With only
        // its last tag byte random, one run in 256 rebuilt the genuine record,
        // which is rightly read.
        if round.is_multiple_of(3) {
            record.truncate(at + 1);
            record.extend(rng.bytes(16));
        }
        assert!(
            record.iter().zip(&genuine).any(|(m, g)| m != g),
            "round {round}: the mutation left the genuine record"
        );
        feed_all(&mut c, &record);
        let got = c.advance(None);
        assert!(
            got.is_err() || c.plain() != b"a genuine record of data",
            "round {round}: a mutated record was read as the real one"
        );
        assert!(
            c.plain().is_empty(),
            "round {round}: data from a mutated record"
        );
    }
}
