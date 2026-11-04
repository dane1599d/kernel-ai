//! Our rustls `CryptoProvider` on the RustCrypto primitives (plan
//! p5-networking, task 4 and spike P5-S1). It only wires audited primitives
//! into rustls's traits, for TLS 1.3 alone:
//! - suites: AES-128-GCM-SHA256, AES-256-GCM-SHA384, ChaCha20-Poly1305-SHA256;
//! - groups: X25519, then P-256;
//! - signatures: ECDSA P-256 and P-384, Ed25519, RSA PKCS#1 v1.5 and PSS.
//!
//! The random source is the caller's: the kernel's RANDOM call once it exists
//! (task 3), RDRAND in `tlsbench` until then, the host's in tests.

mod aead;
mod hash;
mod kx;
mod sign;
mod verify;

use alloc::boxed::Box;
use alloc::vec;
use rustls::crypto::tls13::HkdfUsingHmac;
use rustls::crypto::{CryptoProvider, SecureRandom};
use rustls::{CipherSuite, SupportedCipherSuite, Tls13CipherSuite};

/// AES-GCM's limit on records under one key (RFC 8446 §5.5).
const AES_GCM_RECORDS: u64 = 1 << 24;

pub static TLS13_AES_128_GCM_SHA256: SupportedCipherSuite =
    SupportedCipherSuite::Tls13(&Tls13CipherSuite {
        common: rustls::CipherSuiteCommon {
            suite: CipherSuite::TLS13_AES_128_GCM_SHA256,
            hash_provider: &hash::SHA256,
            confidentiality_limit: AES_GCM_RECORDS,
        },
        hkdf_provider: &HkdfUsingHmac(&hash::HMAC_SHA256),
        aead_alg: &aead::AES_128_GCM,
        quic: None,
    });

pub static TLS13_AES_256_GCM_SHA384: SupportedCipherSuite =
    SupportedCipherSuite::Tls13(&Tls13CipherSuite {
        common: rustls::CipherSuiteCommon {
            suite: CipherSuite::TLS13_AES_256_GCM_SHA384,
            hash_provider: &hash::SHA384,
            confidentiality_limit: AES_GCM_RECORDS,
        },
        hkdf_provider: &HkdfUsingHmac(&hash::HMAC_SHA384),
        aead_alg: &aead::AES_256_GCM,
        quic: None,
    });

pub static TLS13_CHACHA20_POLY1305_SHA256: SupportedCipherSuite =
    SupportedCipherSuite::Tls13(&Tls13CipherSuite {
        common: rustls::CipherSuiteCommon {
            suite: CipherSuite::TLS13_CHACHA20_POLY1305_SHA256,
            hash_provider: &hash::SHA256,
            confidentiality_limit: u64::MAX,
        },
        hkdf_provider: &HkdfUsingHmac(&hash::HMAC_SHA256),
        aead_alg: &aead::CHACHA20_POLY1305,
        quic: None,
    });

/// The provider, drawing its random numbers from `random`. Call it once: its
/// key-exchange groups hold `random` and live as long as the program.
pub fn provider(random: &'static dyn SecureRandom) -> CryptoProvider {
    let x25519: &'static kx::X25519 = Box::leak(Box::new(kx::X25519(random)));
    let p256: &'static kx::P256 = Box::leak(Box::new(kx::P256(random)));
    CryptoProvider {
        cipher_suites: vec![
            TLS13_AES_128_GCM_SHA256,
            TLS13_AES_256_GCM_SHA384,
            TLS13_CHACHA20_POLY1305_SHA256,
        ],
        kx_groups: vec![x25519, p256],
        signature_verification_algorithms: verify::ALGORITHMS,
        secure_random: random,
        key_provider: &sign::Keys,
    }
}
