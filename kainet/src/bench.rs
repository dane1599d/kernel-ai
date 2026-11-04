//! P5-S1's measurements (feature `server`), run alike in ring 3 by `tlsbench`
//! and on the host by `examples/tlsbench.rs`: full handshakes between a client
//! and a server of ours in memory, the AEADs' raw speed, and data through
//! rustls's record layer. The server's certificate is a fixture of the test CA
//! (`fixtures/`), valid from 2025 to 2045.

use crate::tls::{Progress, Tls, TlsError};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use rustls::client::UnbufferedClientConnection;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::UnbufferedServerConnection;
use rustls::time_provider::TimeProvider;
use rustls::{ClientConfig, RootCertStore, ServerConfig, SupportedCipherSuite};

/// The bench CA's certificate, the server's, and the server's key (PKCS#8).
pub const CA: &[u8] = include_bytes!("../fixtures/bench-ca.der");
pub const LEAF: &[u8] = include_bytes!("../fixtures/bench-leaf.der");
pub const LEAF_KEY: &[u8] = include_bytes!("../fixtures/bench-leaf.pk8");
/// The name the server's certificate holds.
pub const NAME: &str = "bench.kai.test";
/// 2030-01-01, inside the fixtures' validity: the bench has no clock of dates.
pub const NOW_S: u64 = 1_893_456_000;

/// A record's worth of data, the most TLS puts in one (2^14 bytes).
const RECORD: usize = 16 * 1024;

#[derive(Debug)]
struct Fixed;

impl TimeProvider for Fixed {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(core::time::Duration::from_secs(
            NOW_S,
        )))
    }
}

/// A monotonic clock in nanoseconds.
pub type Clock<'a> = &'a dyn Fn() -> u64;

fn client(provider: Arc<CryptoProvider>) -> Result<Tls<UnbufferedClientConnection>, TlsError> {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(CA))?;
    let config = ClientConfig::builder_with_details(provider, Arc::new(Fixed))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = ServerName::try_from(NAME).map_err(|_| TlsError::Transport("bad name"))?;
    Ok(Tls::new(UnbufferedClientConnection::new(
        Arc::new(config),
        name,
    )?))
}

fn server(provider: Arc<CryptoProvider>) -> Result<Tls<UnbufferedServerConnection>, TlsError> {
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(LEAF_KEY.to_vec()));
    let config = ServerConfig::builder_with_details(provider, Arc::new(Fixed))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(LEAF)], key)?;
    Ok(Tls::new(UnbufferedServerConnection::new(Arc::new(config))?))
}

/// Moves every outgoing byte of `from` into `to`.
fn carry<A: crate::tls::Side, B: crate::tls::Side>(from: &mut Tls<A>, to: &mut Tls<B>) -> bool {
    let bytes = from.take_outgoing();
    let mut at = 0;
    while at < bytes.len() {
        at += to.feed(&bytes[at..]);
        if to.room() == 0 {
            // A whole record is always under the room; take what came.
            let _ = to.advance(None);
        }
    }
    !bytes.is_empty()
}

/// A connected pair, handshake done.
fn pair(
    provider: &Arc<CryptoProvider>,
) -> Result<
    (
        Tls<UnbufferedClientConnection>,
        Tls<UnbufferedServerConnection>,
    ),
    TlsError,
> {
    let mut c = client(provider.clone())?;
    let mut s = server(provider.clone())?;
    for _ in 0..16 {
        let cp = c.advance(None)?;
        let moved = carry(&mut c, &mut s);
        let sp = s.advance(None)?;
        let back = carry(&mut s, &mut c);
        if cp == Progress::Ready && sp == Progress::Ready && !moved && !back {
            return Ok((c, s));
        }
    }
    Err(TlsError::Transport("the handshake did not finish"))
}

/// The fastest of `n` full handshakes, in nanoseconds: the client's work and
/// the server's together, since both run here.
pub fn handshake_ns(provider: &Arc<CryptoProvider>, n: u32, clock: Clock) -> Result<u64, TlsError> {
    let mut best = u64::MAX;
    for _ in 0..n {
        let start = clock();
        pair(provider)?;
        best = best.min(clock().saturating_sub(start));
    }
    Ok(best)
}

/// `bytes` sent by the server and read by the client over `suite`: MB/s.
pub fn record_mbps(
    provider: &Arc<CryptoProvider>,
    suite: SupportedCipherSuite,
    bytes: usize,
    clock: Clock,
) -> Result<u64, TlsError> {
    let mut only = (**provider).clone();
    only.cipher_suites = vec![suite];
    let (mut c, mut s) = pair(&Arc::new(only))?;
    let chunk = vec![0x5au8; RECORD * 4];
    let mut read = 0;
    let start = clock();
    while read < bytes {
        if s.advance(Some(&chunk))? != Progress::Wrote {
            return Err(TlsError::Transport("the server could not write"));
        }
        let out = s.take_outgoing();
        let mut at = 0;
        while at < out.len() {
            at += c.feed(&out[at..]);
            c.advance(None)?;
            read += c.plain().len();
            let n = c.plain().len();
            c.consume(n);
        }
    }
    Ok(mbps(bytes, clock().saturating_sub(start)))
}

fn mbps(bytes: usize, ns: u64) -> u64 {
    (bytes as u64).saturating_mul(1000) / ns.max(1)
}

/// One AEAD's raw sealing speed over `bytes`, record by record: MB/s.
fn seal_mbps<A: aes_gcm::aead::AeadInOut + aes_gcm::aead::KeyInit>(
    bytes: usize,
    clock: Clock,
) -> u64 {
    let key = vec![7u8; A::key_size()];
    let Ok(cipher) = A::new_from_slice(&key) else {
        return 0;
    };
    let mut record: Vec<u8> = vec![0x5a; RECORD];
    let nonce = aes_gcm::aead::Nonce::<A>::default();
    let start = clock();
    let mut done = 0;
    while done < bytes {
        let _ = cipher.encrypt_inout_detached(&nonce, b"aad", record.as_mut_slice().into());
        done += RECORD;
    }
    mbps(done, clock().saturating_sub(start))
}

/// The AEADs' raw speeds: AES-128-GCM, AES-256-GCM, ChaCha20-Poly1305, MB/s.
pub fn aead_mbps(bytes: usize, clock: Clock) -> [(&'static str, u64); 3] {
    [
        ("AES-128-GCM", seal_mbps::<aes_gcm::Aes128Gcm>(bytes, clock)),
        ("AES-256-GCM", seal_mbps::<aes_gcm::Aes256Gcm>(bytes, clock)),
        (
            "ChaCha20-Poly1305",
            seal_mbps::<chacha20poly1305::ChaCha20Poly1305>(bytes, clock),
        ),
    ]
}
