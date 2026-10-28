//! P5-S1's timings, the same code on the host and in ring 3: a full handshake
//! against the in-memory test server, records through rustls (the server
//! seals, the client opens, as in a download), and the raw AEADs. The clock is
//! the caller's, in nanoseconds.

use crate::fixture::{self, Chain};
use crate::pipe::{self, PipeError};
use crate::sign::P256Key;
use crate::{FixedTime, client_config, server_config, suites};
use aes_gcm::aead::{AeadInOut, KeyInit};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use rustls::client::UnbufferedClientConnection;
use rustls::pki_types::ServerName;
use rustls::server::UnbufferedServerConnection;
use rustls::time_provider::TimeProvider;
use rustls::{CipherSuite, NamedGroup, RootCertStore, SupportedCipherSuite};

pub type Clock = fn() -> u64;

/// A handshake's cost on each side, and what it negotiated.
#[derive(Debug, Clone, Copy)]
pub struct Handshake {
    /// The client's own time: its ClientHello, key share, the chain and
    /// CertificateVerify checks, and its Finished.
    pub client_ns: u64,
    pub server_ns: u64,
    pub suite: Option<CipherSuite>,
    pub group: Option<NamedGroup>,
}

/// Bytes moved through the record layer and each side's time.
#[derive(Debug, Clone, Copy)]
pub struct Transfer {
    pub bytes: usize,
    pub open_ns: u64,
    pub seal_ns: u64,
}

/// At most this many exchanges for a handshake (TLS 1.3 needs two).
const HANDSHAKE_ROUNDS: usize = 8;
/// The server seals this much per turn, like a socket's send window.
const SEND_WINDOW: usize = 256 * 1024;
/// A raw-AEAD record, the TLS maximum.
const RECORD: usize = 16 * 1024;

struct Pair {
    client: UnbufferedClientConnection,
    server: UnbufferedServerConnection,
    to_server: Vec<u8>,
    to_client: Vec<u8>,
}

/// What the client trusts and believes: its roots, the time, the name it asks for.
pub struct View {
    pub roots: RootCertStore,
    pub now: u64,
    pub host: &'static str,
}

impl View {
    pub fn of(chain: &Chain) -> Self {
        Self {
            roots: chain.roots(),
            now: fixture::NOW,
            host: fixture::HOST,
        }
    }
}

fn connect(
    chain: &Chain,
    view: View,
    offered: Vec<SupportedCipherSuite>,
    clock: Clock,
) -> Result<(Pair, Handshake), PipeError> {
    let server_time: Arc<dyn TimeProvider> = Arc::new(FixedTime(fixture::NOW));
    let client_config = client_config(offered, view.roots, Arc::new(FixedTime(view.now)))?;
    let key = P256Key::from_scalar(fixture::LEAF_SCALAR).ok_or(PipeError::Unexpected)?;
    let server_config = server_config(chain.served(), key, server_time)?;
    let name = ServerName::try_from(view.host)
        .map_err(|_| PipeError::Unexpected)?
        .to_owned();

    let start = clock();
    let client = UnbufferedClientConnection::new(client_config, name)?;
    let mut client_ns = clock() - start;
    let mut pair = Pair {
        client,
        server: UnbufferedServerConnection::new(server_config)?,
        to_server: Vec::new(),
        to_client: Vec::new(),
    };
    let mut server_ns = 0;
    let mut sink = |_: &[u8]| {};
    for _ in 0..HANDSHAKE_ROUNDS {
        let start = clock();
        let c = pipe::turn(
            &mut pair.client,
            &mut pair.to_client,
            &mut pair.to_server,
            &mut &[][..],
            0,
            &mut sink,
        )?;
        let middle = clock();
        let s = pipe::turn(
            &mut pair.server,
            &mut pair.to_server,
            &mut pair.to_client,
            &mut &[][..],
            0,
            &mut sink,
        )?;
        client_ns += middle - start;
        server_ns += clock() - middle;
        if c.ready && s.ready && pair.to_client.is_empty() && pair.to_server.is_empty() {
            let handshake = Handshake {
                client_ns,
                server_ns,
                suite: pair.client.negotiated_cipher_suite().map(|s| s.suite()),
                group: pair
                    .client
                    .negotiated_key_exchange_group()
                    .map(|g| g.name()),
            };
            return Ok((pair, handshake));
        }
    }
    Err(PipeError::Unexpected)
}

/// One full handshake against the test server with `chain`.
pub fn handshake(chain: &Chain, clock: Clock) -> Result<Handshake, PipeError> {
    handshake_as(chain, View::of(chain), clock)
}

/// A handshake with the client's `view`, to show what it refuses.
pub fn handshake_as(chain: &Chain, view: View, clock: Clock) -> Result<Handshake, PipeError> {
    connect(chain, view, suites(), clock).map(|(_, handshake)| handshake)
}

/// `bytes` from the server to the client over `suite`; the client checks it
/// got every byte back in order.
pub fn transfer(
    suite: SupportedCipherSuite,
    bytes: usize,
    clock: Clock,
) -> Result<Transfer, PipeError> {
    let (mut pair, _) = connect(
        &fixture::ECDSA,
        View::of(&fixture::ECDSA),
        vec![suite],
        clock,
    )?;
    let data: Vec<u8> = (0..bytes).map(|i| (i % 251) as u8).collect();
    let mut send = &data[..];
    // The client copies each record out, as a download fills its chunk buffer;
    // the bytes are compared after the timing.
    let mut got: Vec<u8> = Vec::with_capacity(bytes);
    let (mut open_ns, mut seal_ns) = (0, 0);
    while got.len() < bytes {
        let start = clock();
        pipe::turn(
            &mut pair.server,
            &mut pair.to_server,
            &mut pair.to_client,
            &mut send,
            SEND_WINDOW,
            &mut |_| {},
        )?;
        let middle = clock();
        pipe::turn(
            &mut pair.client,
            &mut pair.to_client,
            &mut pair.to_server,
            &mut &[][..],
            0,
            &mut |payload| got.extend_from_slice(payload),
        )?;
        seal_ns += middle - start;
        open_ns += clock() - middle;
        if pair.to_client.is_empty() && send.is_empty() && got.len() < bytes {
            return Err(PipeError::Unexpected);
        }
    }
    if got != data {
        return Err(PipeError::Unexpected);
    }
    Ok(Transfer {
        bytes,
        open_ns,
        seal_ns,
    })
}

/// Seals `bytes` in 16 KiB records with the bare AEAD; returns the time.
pub fn raw_seal<A: AeadInOut + KeyInit>(bytes: usize, clock: Clock) -> u64 {
    let key = vec![7u8; A::key_size()];
    let cipher = A::new_from_slice(&key).expect("key length");
    let nonce = aes_gcm::aead::Nonce::<A>::default();
    let mut record = vec![0x5au8; RECORD];
    let start = clock();
    for _ in 0..bytes / RECORD {
        let tag = cipher.encrypt_inout_detached(&nonce, b"hdr", record.as_mut_slice().into());
        core::hint::black_box(&tag);
    }
    core::hint::black_box(&record);
    clock() - start
}

/// Megabytes (10^6 bytes) a second.
pub fn mb_per_s(bytes: usize, ns: u64) -> u64 {
    (bytes as u64).saturating_mul(1000) / ns.max(1)
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::{TLS13_AES_128_GCM_SHA256, TLS13_CHACHA20_POLY1305_SHA256};

    fn now() -> u64 {
        use std::sync::OnceLock;
        use std::time::Instant;
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_nanos() as u64
    }

    #[test]
    fn both_fixture_chains_handshake_on_tls13_with_x25519() {
        for chain in [&fixture::ECDSA, &fixture::RSA] {
            let h = handshake(chain, now).expect(chain.name);
            assert_eq!(h.suite, Some(CipherSuite::TLS13_AES_128_GCM_SHA256));
            assert_eq!(h.group, Some(NamedGroup::X25519));
        }
    }

    fn refused(chain: &Chain, view: View) -> rustls::Error {
        match handshake_as(chain, view, now) {
            Err(PipeError::Tls(error)) => error,
            other => panic!("expected a TLS error, got {other:?}"),
        }
    }

    #[test]
    fn wrong_name_expired_unknown_ca_and_not_yet_valid_are_refused() {
        use rustls::{CertificateError, Error};
        let chain = &fixture::ECDSA;
        let wrong = View {
            host: "other.kai.test",
            ..View::of(chain)
        };
        assert!(matches!(
            refused(chain, wrong),
            Error::InvalidCertificate(CertificateError::NotValidForNameContext { .. })
        ));
        // 2037-01-01, after the fixtures' ten years.
        let late = View {
            now: 2_114_380_800,
            ..View::of(chain)
        };
        assert!(matches!(
            refused(chain, late),
            Error::InvalidCertificate(CertificateError::ExpiredContext { .. })
        ));
        // 2026-01-01, before they were made.
        let early = View {
            now: 1_767_225_600,
            ..View::of(chain)
        };
        assert!(matches!(
            refused(chain, early),
            Error::InvalidCertificate(CertificateError::NotValidYetContext { .. })
        ));
        // The RSA chain's root does not vouch for the ECDSA chain.
        let stranger = View {
            roots: fixture::RSA.roots(),
            ..View::of(chain)
        };
        assert!(matches!(
            refused(chain, stranger),
            Error::InvalidCertificate(CertificateError::UnknownIssuer)
        ));
        // Mozilla's roots do not hold the test CA either.
        let mozilla = View {
            roots: crate::mozilla_roots(),
            ..View::of(chain)
        };
        assert!(matches!(
            refused(chain, mozilla),
            Error::InvalidCertificate(CertificateError::UnknownIssuer)
        ));
    }

    #[test]
    fn every_suite_carries_data_intact() {
        for suite in suites() {
            let t = transfer(suite, 1 << 20, now).expect("transfer");
            assert_eq!(t.bytes, 1 << 20);
        }
    }

    #[test]
    fn raw_aeads_run() {
        assert!(raw_seal::<aes_gcm::Aes128Gcm>(1 << 20, now) > 0);
        assert!(raw_seal::<chacha20poly1305::ChaCha20Poly1305>(1 << 20, now) > 0);
        let _ = (&TLS13_AES_128_GCM_SHA256, &TLS13_CHACHA20_POLY1305_SHA256);
    }
}
