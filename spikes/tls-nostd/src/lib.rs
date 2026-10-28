//! Spike P5-S1 `tls-nostd` (docs/plans/p5-networking.md §5): rustls 0.23.46
//! with no default features, so no std, no aws-lc-rs and no ring, on our own
//! `CryptoProvider` over the RustCrypto crates. TLS 1.3 only. Not for merge:
//! `kainet` (P5 task 4) is written fresh from what this spike measures.
//!
//! - `hash`, `aead`, `kx`, `verify`, `random`: the provider;
//! - `sign`: a P-256 signer for the in-memory test server only;
//! - `pipe`: drives rustls's unbuffered client and server over byte buffers;
//! - `bench`: the handshake and record timings `tlsbench` prints in ring 3.

#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod aead;
pub mod bench;
pub mod hash;
pub mod kx;
pub mod pipe;
pub mod random;
pub mod report;
pub mod sign;
pub mod verify;

use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::time::Duration;
use rustls::client::ClientConfig;
use rustls::crypto::tls13::HkdfUsingHmac;
use rustls::crypto::{CryptoProvider, KeyProvider};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use rustls::server::ServerConfig;
use rustls::sign::{CertifiedKey, SigningKey, SingleCertAndKey};
use rustls::time_provider::TimeProvider;
use rustls::{
    CipherSuite, CipherSuiteCommon, Error, RootCertStore, SupportedCipherSuite, Tls13CipherSuite,
};

/// RFC 8446 §5.5's AES-GCM limit; ChaCha20-Poly1305 has none that matters.
const GCM_RECORDS: u64 = 1 << 24;

pub static TLS13_AES_128_GCM_SHA256: Tls13CipherSuite = Tls13CipherSuite {
    common: CipherSuiteCommon {
        suite: CipherSuite::TLS13_AES_128_GCM_SHA256,
        hash_provider: &hash::Sha256Hash,
        confidentiality_limit: GCM_RECORDS,
    },
    hkdf_provider: &HkdfUsingHmac(&hash::Sha256Hmac),
    aead_alg: &aead::Aes128,
    quic: None,
};

pub static TLS13_AES_256_GCM_SHA384: Tls13CipherSuite = Tls13CipherSuite {
    common: CipherSuiteCommon {
        suite: CipherSuite::TLS13_AES_256_GCM_SHA384,
        hash_provider: &hash::Sha384Hash,
        confidentiality_limit: GCM_RECORDS,
    },
    hkdf_provider: &HkdfUsingHmac(&hash::Sha384Hmac),
    aead_alg: &aead::Aes256,
    quic: None,
};

pub static TLS13_CHACHA20_POLY1305_SHA256: Tls13CipherSuite = Tls13CipherSuite {
    common: CipherSuiteCommon {
        suite: CipherSuite::TLS13_CHACHA20_POLY1305_SHA256,
        hash_provider: &hash::Sha256Hash,
        confidentiality_limit: u64::MAX,
    },
    hkdf_provider: &HkdfUsingHmac(&hash::Sha256Hmac),
    aead_alg: &aead::ChaCha,
    quic: None,
};

/// The three TLS 1.3 suites, AES-GCM first as AES-NI is everywhere we run.
pub fn suites() -> Vec<SupportedCipherSuite> {
    vec![
        SupportedCipherSuite::Tls13(&TLS13_AES_128_GCM_SHA256),
        SupportedCipherSuite::Tls13(&TLS13_AES_256_GCM_SHA384),
        SupportedCipherSuite::Tls13(&TLS13_CHACHA20_POLY1305_SHA256),
    ]
}

/// The client loads no private keys.
#[derive(Debug)]
struct NoKeys;

impl KeyProvider for NoKeys {
    fn load_private_key(&self, _key: PrivateKeyDer<'static>) -> Result<Arc<dyn SigningKey>, Error> {
        Err(Error::General("this provider loads no private keys".into()))
    }
}

/// Our provider, offering `suites`.
pub fn provider(suites: Vec<SupportedCipherSuite>) -> CryptoProvider {
    CryptoProvider {
        cipher_suites: suites,
        kx_groups: vec![&kx::X25519, &kx::P256, &kx::P384],
        signature_verification_algorithms: verify::ALGORITHMS,
        secure_random: &random::Random,
        key_provider: &NoKeys,
    }
}

/// A clock fixed at one Unix time: ring 3 has no wall clock yet (risk table:
/// time will come from Limine's boot date plus the monotonic clock).
#[derive(Debug)]
pub struct FixedTime(pub u64);

impl TimeProvider for FixedTime {
    fn current_time(&self) -> Option<UnixTime> {
        Some(UnixTime::since_unix_epoch(Duration::from_secs(self.0)))
    }
}

/// Mozilla's roots, from webpki-roots.
pub fn mozilla_roots() -> RootCertStore {
    RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    }
}

/// A TLS 1.3-only client config on our provider.
pub fn client_config(
    suites: Vec<SupportedCipherSuite>,
    roots: RootCertStore,
    time: Arc<dyn TimeProvider>,
) -> Result<Arc<ClientConfig>, Error> {
    let config = ClientConfig::builder_with_details(Arc::new(provider(suites)), time)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// The test server's config: one chain, the P-256 leaf key, no tickets.
pub fn server_config(
    chain: Vec<CertificateDer<'static>>,
    key: Arc<dyn SigningKey>,
    time: Arc<dyn TimeProvider>,
) -> Result<Arc<ServerConfig>, Error> {
    let resolver = SingleCertAndKey::from(CertifiedKey::new(chain, key));
    let mut config = ServerConfig::builder_with_details(Arc::new(provider(suites())), time)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(resolver));
    config.send_tls13_tickets = 0;
    Ok(Arc::new(config))
}

/// The test PKI (fixtures/, made with openssl 3.5.4 on 2026-10-28, valid ten
/// years): an ECDSA chain shaped like github.com's (P-384 root, P-256
/// intermediate signed with SHA-384, P-256 leaf) and an RSA-2048 chain shaped
/// like huggingface.co's, both ending in the same P-256 leaf key for
/// `mock.kai.test`. The leaf's scalar is a test key and protects nothing.
pub mod fixture {
    use super::*;

    pub const HOST: &str = "mock.kai.test";
    /// 2027-01-15, inside every fixture's validity.
    pub const NOW: u64 = 1_800_000_000;
    pub const LEAF_SCALAR: &[u8] = include_bytes!("../fixtures/leaf.scalar");

    pub struct Chain {
        pub name: &'static str,
        pub root: &'static [u8],
        pub intermediate: &'static [u8],
        pub leaf: &'static [u8],
    }

    pub const ECDSA: Chain = Chain {
        name: "ECDSA P-384/P-256 chain",
        root: include_bytes!("../fixtures/ec-root.der"),
        intermediate: include_bytes!("../fixtures/ec-int.der"),
        leaf: include_bytes!("../fixtures/ec-leaf.der"),
    };

    pub const RSA: Chain = Chain {
        name: "RSA-2048 chain",
        root: include_bytes!("../fixtures/rsa-root.der"),
        intermediate: include_bytes!("../fixtures/rsa-int.der"),
        leaf: include_bytes!("../fixtures/rsa-leaf.der"),
    };

    impl Chain {
        pub fn roots(&self) -> RootCertStore {
            let mut roots = RootCertStore::empty();
            // The fixture root parses; a failure shows as a handshake error.
            let _ = roots.add(CertificateDer::from(self.root));
            roots
        }

        pub fn served(&self) -> Vec<CertificateDer<'static>> {
            vec![
                CertificateDer::from(self.leaf),
                CertificateDer::from(self.intermediate),
            ]
        }
    }
}
