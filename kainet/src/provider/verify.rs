//! Signature checks, for certificates (through rustls-webpki) and for the
//! server's handshake signature: ECDSA on P-256 and P-384, Ed25519, and RSA
//! PKCS#1 v1.5 and PSS with keys of 2048 to 8192 bits. Only public keys are
//! used, so RSA's private-key timing advisory (RUSTSEC-2023-0071) does not
//! apply.

use pki_types::{AlgorithmIdentifier, InvalidSignature, SignatureVerificationAlgorithm, alg_id};
use rsa::traits::PublicKeyParts;
use rustls::SignatureScheme;
use rustls::crypto::WebPkiSupportedAlgorithms;

/// The RSA moduli we take, in bits, as rustls's own providers do.
const RSA_BITS: core::ops::RangeInclusive<usize> = 2048..=8192;

#[derive(Debug, Clone, Copy)]
enum Hash {
    Sha256,
    Sha384,
    Sha512,
}

#[derive(Debug, Clone, Copy)]
enum Curve {
    P256,
    P384,
}

#[derive(Debug)]
struct Ecdsa {
    curve: Curve,
    hash: Hash,
    public: AlgorithmIdentifier,
    signature: AlgorithmIdentifier,
}

/// `message`'s digest under `hash`, into `out`; its length.
fn digest(hash: Hash, message: &[u8], out: &mut [u8; 64]) -> usize {
    use sha2::Digest;
    let bytes = match hash {
        Hash::Sha256 => &sha2::Sha256::digest(message)[..],
        Hash::Sha384 => &sha2::Sha384::digest(message)[..],
        Hash::Sha512 => &sha2::Sha512::digest(message)[..],
    };
    out[..bytes.len()].copy_from_slice(bytes);
    bytes.len()
}

impl SignatureVerificationAlgorithm for Ecdsa {
    fn verify_signature(
        &self,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), InvalidSignature> {
        use p256::ecdsa::signature::hazmat::PrehashVerifier;
        let mut prehash = [0u8; 64];
        let len = digest(self.hash, message, &mut prehash);
        let prehash = &prehash[..len];
        let checked = match self.curve {
            Curve::P256 => {
                let key = p256::ecdsa::VerifyingKey::from_sec1_bytes(public_key)
                    .map_err(|_| InvalidSignature)?;
                let signature =
                    p256::ecdsa::Signature::from_der(signature).map_err(|_| InvalidSignature)?;
                key.verify_prehash(prehash, &signature)
            }
            Curve::P384 => {
                let key = p384::ecdsa::VerifyingKey::from_sec1_bytes(public_key)
                    .map_err(|_| InvalidSignature)?;
                let signature =
                    p384::ecdsa::Signature::from_der(signature).map_err(|_| InvalidSignature)?;
                key.verify_prehash(prehash, &signature)
            }
        };
        checked.map_err(|_| InvalidSignature)
    }

    fn public_key_alg_id(&self) -> AlgorithmIdentifier {
        self.public
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
        let key: &[u8; 32] = public_key.try_into().map_err(|_| InvalidSignature)?;
        let key = ed25519_dalek::VerifyingKey::from_bytes(key).map_err(|_| InvalidSignature)?;
        let signature =
            ed25519_dalek::Signature::from_slice(signature).map_err(|_| InvalidSignature)?;
        key.verify_strict(message, &signature)
            .map_err(|_| InvalidSignature)
    }

    fn public_key_alg_id(&self) -> AlgorithmIdentifier {
        alg_id::ED25519
    }

    fn signature_alg_id(&self) -> AlgorithmIdentifier {
        alg_id::ED25519
    }
}

#[derive(Debug, Clone, Copy)]
enum Padding {
    Pkcs1,
    Pss,
}

#[derive(Debug)]
struct Rsa {
    padding: Padding,
    hash: Hash,
    signature: AlgorithmIdentifier,
}

impl SignatureVerificationAlgorithm for Rsa {
    fn verify_signature(
        &self,
        public_key: &[u8],
        message: &[u8],
        signature: &[u8],
    ) -> Result<(), InvalidSignature> {
        use rsa::pkcs1::DecodeRsaPublicKey;
        use rsa::sha2::{Digest, Sha256, Sha384, Sha512};
        use rsa::{Pkcs1v15Sign, Pss};
        let key = rsa::RsaPublicKey::from_pkcs1_der(public_key).map_err(|_| InvalidSignature)?;
        if !RSA_BITS.contains(&key.n().bits()) {
            return Err(InvalidSignature);
        }
        let checked = match (self.padding, self.hash) {
            (Padding::Pkcs1, Hash::Sha256) => key.verify(
                Pkcs1v15Sign::new::<Sha256>(),
                &Sha256::digest(message),
                signature,
            ),
            (Padding::Pkcs1, Hash::Sha384) => key.verify(
                Pkcs1v15Sign::new::<Sha384>(),
                &Sha384::digest(message),
                signature,
            ),
            (Padding::Pkcs1, Hash::Sha512) => key.verify(
                Pkcs1v15Sign::new::<Sha512>(),
                &Sha512::digest(message),
                signature,
            ),
            (Padding::Pss, Hash::Sha256) => {
                key.verify(Pss::new::<Sha256>(), &Sha256::digest(message), signature)
            }
            (Padding::Pss, Hash::Sha384) => {
                key.verify(Pss::new::<Sha384>(), &Sha384::digest(message), signature)
            }
            (Padding::Pss, Hash::Sha512) => {
                key.verify(Pss::new::<Sha512>(), &Sha512::digest(message), signature)
            }
        };
        checked.map_err(|_| InvalidSignature)
    }

    fn public_key_alg_id(&self) -> AlgorithmIdentifier {
        alg_id::RSA_ENCRYPTION
    }

    fn signature_alg_id(&self) -> AlgorithmIdentifier {
        self.signature
    }
}

static ECDSA_P256_SHA256: Ecdsa = Ecdsa {
    curve: Curve::P256,
    hash: Hash::Sha256,
    public: alg_id::ECDSA_P256,
    signature: alg_id::ECDSA_SHA256,
};
static ECDSA_P256_SHA384: Ecdsa = Ecdsa {
    curve: Curve::P256,
    hash: Hash::Sha384,
    public: alg_id::ECDSA_P256,
    signature: alg_id::ECDSA_SHA384,
};
static ECDSA_P384_SHA256: Ecdsa = Ecdsa {
    curve: Curve::P384,
    hash: Hash::Sha256,
    public: alg_id::ECDSA_P384,
    signature: alg_id::ECDSA_SHA256,
};
static ECDSA_P384_SHA384: Ecdsa = Ecdsa {
    curve: Curve::P384,
    hash: Hash::Sha384,
    public: alg_id::ECDSA_P384,
    signature: alg_id::ECDSA_SHA384,
};
static ED25519: Ed25519 = Ed25519;
static RSA_PKCS1_SHA256: Rsa = Rsa {
    padding: Padding::Pkcs1,
    hash: Hash::Sha256,
    signature: alg_id::RSA_PKCS1_SHA256,
};
static RSA_PKCS1_SHA384: Rsa = Rsa {
    padding: Padding::Pkcs1,
    hash: Hash::Sha384,
    signature: alg_id::RSA_PKCS1_SHA384,
};
static RSA_PKCS1_SHA512: Rsa = Rsa {
    padding: Padding::Pkcs1,
    hash: Hash::Sha512,
    signature: alg_id::RSA_PKCS1_SHA512,
};
static RSA_PSS_SHA256: Rsa = Rsa {
    padding: Padding::Pss,
    hash: Hash::Sha256,
    signature: alg_id::RSA_PSS_SHA256,
};
static RSA_PSS_SHA384: Rsa = Rsa {
    padding: Padding::Pss,
    hash: Hash::Sha384,
    signature: alg_id::RSA_PSS_SHA384,
};
static RSA_PSS_SHA512: Rsa = Rsa {
    padding: Padding::Pss,
    hash: Hash::Sha512,
    signature: alg_id::RSA_PSS_SHA512,
};

/// What webpki and the handshake may check. TLS 1.3 never takes PKCS#1 v1.5
/// for the handshake itself; rustls refuses it there, and certificates may
/// still be signed with it.
pub static ALGORITHMS: WebPkiSupportedAlgorithms = WebPkiSupportedAlgorithms {
    all: &[
        &ECDSA_P256_SHA256,
        &ECDSA_P256_SHA384,
        &ECDSA_P384_SHA256,
        &ECDSA_P384_SHA384,
        &ED25519,
        &RSA_PKCS1_SHA256,
        &RSA_PKCS1_SHA384,
        &RSA_PKCS1_SHA512,
        &RSA_PSS_SHA256,
        &RSA_PSS_SHA384,
        &RSA_PSS_SHA512,
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
