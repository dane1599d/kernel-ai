//! Signature checks for certificate chains (rustls-webpki) and TLS 1.3's
//! CertificateVerify: ECDSA P-256 and P-384, RSA PKCS#1 v1.5 and PSS, and
//! Ed25519. `public_key` is the SubjectPublicKeyInfo's key bits, as webpki
//! passes it: a SEC1 point, a PKCS#1 RSAPublicKey, or 32 Ed25519 bytes.

use core::sync::atomic::{AtomicU32, Ordering};
use rustls::SignatureScheme;
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{
    AlgorithmIdentifier, InvalidSignature, SignatureVerificationAlgorithm, alg_id,
};

/// How often each algorithm ran, for `tlsprobe`'s report of what real hosts use.
pub static COUNTS: [AtomicU32; 9] = [const { AtomicU32::new(0) }; 9];
pub const NAMES: [&str; 9] = [
    "ecdsa-p256-sha256",
    "ecdsa-p256-sha384",
    "ecdsa-p384-sha256",
    "ecdsa-p384-sha384",
    "rsa-pkcs1-sha256",
    "rsa-pkcs1-sha384",
    "rsa-pkcs1-sha512",
    "rsa-pss-sha256/384/512",
    "ed25519",
];

fn count(index: usize) {
    COUNTS[index].fetch_add(1, Ordering::Relaxed);
}

#[derive(Debug)]
struct Ecdsa {
    curve: Curve,
    hash: Hash,
    public: AlgorithmIdentifier,
    signature: AlgorithmIdentifier,
    index: usize,
}

#[derive(Debug, Clone, Copy)]
enum Curve {
    P256,
    P384,
}

#[derive(Debug, Clone, Copy)]
enum Hash {
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    fn digest(self, message: &[u8], out: &mut [u8; 64]) -> usize {
        use sha2::Digest;
        match self {
            Hash::Sha256 => {
                out[..32].copy_from_slice(&sha2::Sha256::digest(message));
                32
            }
            Hash::Sha384 => {
                out[..48].copy_from_slice(&sha2::Sha384::digest(message));
                48
            }
            Hash::Sha512 => {
                out.copy_from_slice(&sha2::Sha512::digest(message));
                64
            }
        }
    }
}

impl SignatureVerificationAlgorithm for Ecdsa {
    fn verify_signature(
        &self,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), InvalidSignature> {
        use ecdsa::signature::hazmat::PrehashVerifier;
        count(self.index);
        let mut digest = [0u8; 64];
        let len = self.hash.digest(message, &mut digest);
        let prehash = &digest[..len];
        match self.curve {
            Curve::P256 => {
                let key = p256::ecdsa::VerifyingKey::from_sec1_bytes(public_key)
                    .map_err(|_| InvalidSignature)?;
                let sig =
                    p256::ecdsa::Signature::from_der(signature).map_err(|_| InvalidSignature)?;
                key.verify_prehash(prehash, &sig)
                    .map_err(|_| InvalidSignature)
            }
            Curve::P384 => {
                let key = p384::ecdsa::VerifyingKey::from_sec1_bytes(public_key)
                    .map_err(|_| InvalidSignature)?;
                let sig =
                    p384::ecdsa::Signature::from_der(signature).map_err(|_| InvalidSignature)?;
                key.verify_prehash(prehash, &sig)
                    .map_err(|_| InvalidSignature)
            }
        }
    }

    fn public_key_alg_id(&self) -> AlgorithmIdentifier {
        self.public
    }

    fn signature_alg_id(&self) -> AlgorithmIdentifier {
        self.signature
    }
}

#[derive(Debug)]
struct Rsa {
    pss: bool,
    hash: Hash,
    signature: AlgorithmIdentifier,
    index: usize,
}

/// RSA keys under 2,048 bits are refused, as webpki's own providers do.
const RSA_MIN_BITS: usize = 2048;
const RSA_MAX_BITS: usize = 8192;

impl SignatureVerificationAlgorithm for Rsa {
    fn verify_signature(
        &self,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), InvalidSignature> {
        use rsa::pkcs1::DecodeRsaPublicKey;
        use rsa::signature::Verifier;
        use rsa::traits::PublicKeyParts;
        use sha2_10::{Sha256, Sha384, Sha512};
        count(self.index);
        let key = rsa::RsaPublicKey::from_pkcs1_der(public_key).map_err(|_| InvalidSignature)?;
        let bits = key.n().bits();
        if !(RSA_MIN_BITS..=RSA_MAX_BITS).contains(&bits) {
            return Err(InvalidSignature);
        }
        macro_rules! check {
            ($module:ident, $digest:ty) => {{
                let sig =
                    rsa::$module::Signature::try_from(signature).map_err(|_| InvalidSignature)?;
                rsa::$module::VerifyingKey::<$digest>::new(key)
                    .verify(message, &sig)
                    .map_err(|_| InvalidSignature)
            }};
        }
        match (self.pss, self.hash) {
            (false, Hash::Sha256) => check!(pkcs1v15, Sha256),
            (false, Hash::Sha384) => check!(pkcs1v15, Sha384),
            (false, Hash::Sha512) => check!(pkcs1v15, Sha512),
            (true, Hash::Sha256) => check!(pss, Sha256),
            (true, Hash::Sha384) => check!(pss, Sha384),
            (true, Hash::Sha512) => check!(pss, Sha512),
        }
    }

    fn public_key_alg_id(&self) -> AlgorithmIdentifier {
        alg_id::RSA_ENCRYPTION
    }

    fn signature_alg_id(&self) -> AlgorithmIdentifier {
        self.signature
    }
}

#[derive(Debug)]
struct Ed25519;

impl SignatureVerificationAlgorithm for Ed25519 {
    fn verify_signature(
        &self,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), InvalidSignature> {
        count(8);
        let key: [u8; 32] = public_key.try_into().map_err(|_| InvalidSignature)?;
        let key = ed25519_dalek::VerifyingKey::from_bytes(&key).map_err(|_| InvalidSignature)?;
        let sig = ed25519_dalek::Signature::from_slice(signature).map_err(|_| InvalidSignature)?;
        key.verify_strict(message, &sig)
            .map_err(|_| InvalidSignature)
    }

    fn public_key_alg_id(&self) -> AlgorithmIdentifier {
        alg_id::ED25519
    }

    fn signature_alg_id(&self) -> AlgorithmIdentifier {
        alg_id::ED25519
    }
}

static ECDSA_P256_SHA256: Ecdsa = Ecdsa {
    curve: Curve::P256,
    hash: Hash::Sha256,
    public: alg_id::ECDSA_P256,
    signature: alg_id::ECDSA_SHA256,
    index: 0,
};
static ECDSA_P256_SHA384: Ecdsa = Ecdsa {
    curve: Curve::P256,
    hash: Hash::Sha384,
    public: alg_id::ECDSA_P256,
    signature: alg_id::ECDSA_SHA384,
    index: 1,
};
static ECDSA_P384_SHA256: Ecdsa = Ecdsa {
    curve: Curve::P384,
    hash: Hash::Sha256,
    public: alg_id::ECDSA_P384,
    signature: alg_id::ECDSA_SHA256,
    index: 2,
};
static ECDSA_P384_SHA384: Ecdsa = Ecdsa {
    curve: Curve::P384,
    hash: Hash::Sha384,
    public: alg_id::ECDSA_P384,
    signature: alg_id::ECDSA_SHA384,
    index: 3,
};
static RSA_PKCS1_SHA256: Rsa = Rsa {
    pss: false,
    hash: Hash::Sha256,
    signature: alg_id::RSA_PKCS1_SHA256,
    index: 4,
};
static RSA_PKCS1_SHA384: Rsa = Rsa {
    pss: false,
    hash: Hash::Sha384,
    signature: alg_id::RSA_PKCS1_SHA384,
    index: 5,
};
static RSA_PKCS1_SHA512: Rsa = Rsa {
    pss: false,
    hash: Hash::Sha512,
    signature: alg_id::RSA_PKCS1_SHA512,
    index: 6,
};
static RSA_PSS_SHA256: Rsa = Rsa {
    pss: true,
    hash: Hash::Sha256,
    signature: alg_id::RSA_PSS_SHA256,
    index: 7,
};
static RSA_PSS_SHA384: Rsa = Rsa {
    pss: true,
    hash: Hash::Sha384,
    signature: alg_id::RSA_PSS_SHA384,
    index: 7,
};
static RSA_PSS_SHA512: Rsa = Rsa {
    pss: true,
    hash: Hash::Sha512,
    signature: alg_id::RSA_PSS_SHA512,
    index: 7,
};
static ED25519: Ed25519 = Ed25519;

/// Every algorithm for chains; TLS 1.3's schemes for CertificateVerify.
pub static ALGORITHMS: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
    all: &[
        &ECDSA_P256_SHA256,
        &ECDSA_P256_SHA384,
        &ECDSA_P384_SHA256,
        &ECDSA_P384_SHA384,
        &RSA_PKCS1_SHA256,
        &RSA_PKCS1_SHA384,
        &RSA_PKCS1_SHA512,
        &RSA_PSS_SHA256,
        &RSA_PSS_SHA384,
        &RSA_PSS_SHA512,
        &ED25519,
    ],
    mapping: &[
        (
            SignatureScheme::ECDSA_NISTP384_SHA384,
            &[&ECDSA_P384_SHA384, &ECDSA_P256_SHA384],
        ),
        (
            SignatureScheme::ECDSA_NISTP256_SHA256,
            &[&ECDSA_P256_SHA256, &ECDSA_P384_SHA256],
        ),
        (SignatureScheme::ED25519, &[&ED25519]),
        (SignatureScheme::RSA_PSS_SHA512, &[&RSA_PSS_SHA512]),
        (SignatureScheme::RSA_PSS_SHA384, &[&RSA_PSS_SHA384]),
        (SignatureScheme::RSA_PSS_SHA256, &[&RSA_PSS_SHA256]),
        (SignatureScheme::RSA_PKCS1_SHA512, &[&RSA_PKCS1_SHA512]),
        (SignatureScheme::RSA_PKCS1_SHA384, &[&RSA_PKCS1_SHA384]),
        (SignatureScheme::RSA_PKCS1_SHA256, &[&RSA_PKCS1_SHA256]),
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    const RSA_KEY: &[u8] = include_bytes!("../fixtures/rsa-int.pub.der");
    const RSA_MSG: &[u8] = include_bytes!("../fixtures/rsa-msg.bin");

    #[test]
    fn rsa_pkcs1_and_pss_vectors_from_openssl_verify() {
        let pkcs1 = include_bytes!("../fixtures/rsa-pkcs1-sha256.sig");
        let pss = include_bytes!("../fixtures/rsa-pss-sha256.sig");
        assert!(
            RSA_PKCS1_SHA256
                .verify_signature(RSA_KEY, RSA_MSG, pkcs1)
                .is_ok()
        );
        assert!(
            RSA_PSS_SHA256
                .verify_signature(RSA_KEY, RSA_MSG, pss)
                .is_ok()
        );
        assert!(
            RSA_PSS_SHA256
                .verify_signature(RSA_KEY, RSA_MSG, pkcs1)
                .is_err()
        );
        assert!(
            RSA_PKCS1_SHA384
                .verify_signature(RSA_KEY, RSA_MSG, pkcs1)
                .is_err()
        );
        let mut flipped = *pss;
        flipped[100] ^= 1;
        assert!(
            RSA_PSS_SHA256
                .verify_signature(RSA_KEY, RSA_MSG, &flipped)
                .is_err()
        );
        assert!(
            RSA_PSS_SHA256
                .verify_signature(RSA_KEY, b"another message", pss)
                .is_err()
        );
    }

    #[test]
    fn every_scheme_maps_to_listed_algorithms() {
        for (_, algs) in ALGORITHMS.mapping {
            for alg in *algs {
                let alg = *alg as *const dyn SignatureVerificationAlgorithm as *const ();
                assert!(
                    ALGORITHMS
                        .all
                        .iter()
                        .any(|a| core::ptr::eq(*a as *const _ as *const (), alg))
                );
            }
        }
    }
}
