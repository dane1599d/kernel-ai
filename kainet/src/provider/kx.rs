//! Key exchange: X25519 (RFC 7748) and P-256 ECDHE with uncompressed points
//! (RFC 8446 §4.2.8.2). Secrets come from the provider's random source and
//! are wiped when dropped.

use alloc::boxed::Box;
use rustls::crypto::{ActiveKeyExchange, SecureRandom, SharedSecret, SupportedKxGroup};
use rustls::{Error, NamedGroup, PeerMisbehaved};
use zeroize::Zeroize;

/// Tries at a P-256 scalar before giving up: each fails with probability
/// about 2^-32, when the random bytes are not below the group order.
const P256_TRIES: usize = 8;

fn invalid_share() -> Error {
    Error::PeerMisbehaved(PeerMisbehaved::InvalidKeyShare)
}

fn random<const N: usize>(random: &dyn SecureRandom) -> Result<[u8; N], Error> {
    let mut bytes = [0u8; N];
    random
        .fill(&mut bytes)
        .map_err(|_| Error::FailedToGetRandomBytes)?;
    Ok(bytes)
}

#[derive(Debug)]
pub struct X25519(pub &'static dyn SecureRandom);

impl SupportedKxGroup for X25519 {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        let mut seed = random::<32>(self.0)?;
        let secret = x25519_dalek::StaticSecret::from(seed);
        seed.zeroize();
        let public = x25519_dalek::PublicKey::from(&secret).to_bytes();
        Ok(Box::new(X25519Share { secret, public }))
    }

    fn name(&self) -> NamedGroup {
        NamedGroup::X25519
    }
}

struct X25519Share {
    secret: x25519_dalek::StaticSecret,
    public: [u8; 32],
}

impl ActiveKeyExchange for X25519Share {
    fn complete(self: Box<Self>, peer: &[u8]) -> Result<SharedSecret, Error> {
        let peer: [u8; 32] = peer.try_into().map_err(|_| invalid_share())?;
        let shared = self
            .secret
            .diffie_hellman(&x25519_dalek::PublicKey::from(peer));
        // A low-order point gives an all-zero secret (RFC 7748 §6.1).
        if !shared.was_contributory() {
            return Err(invalid_share());
        }
        Ok(SharedSecret::from(&shared.as_bytes()[..]))
    }

    fn pub_key(&self) -> &[u8] {
        &self.public
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::X25519
    }
}

#[derive(Debug)]
pub struct P256(pub &'static dyn SecureRandom);

impl SupportedKxGroup for P256 {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        for _ in 0..P256_TRIES {
            let mut bytes = random::<32>(self.0)?;
            let secret = p256::SecretKey::from_slice(&bytes);
            bytes.zeroize();
            if let Ok(secret) = secret {
                let public = secret.public_key().to_sec1_bytes();
                return Ok(Box::new(P256Share { secret, public }));
            }
        }
        Err(Error::FailedToGetRandomBytes)
    }

    fn name(&self) -> NamedGroup {
        NamedGroup::secp256r1
    }
}

struct P256Share {
    secret: p256::SecretKey,
    public: Box<[u8]>,
}

/// An uncompressed SEC1 point on P-256: 0x04, then x and y.
const P256_POINT: usize = 65;

impl ActiveKeyExchange for P256Share {
    fn complete(self: Box<Self>, peer: &[u8]) -> Result<SharedSecret, Error> {
        // TLS 1.3 takes only the uncompressed form; from_sec1_bytes refuses a
        // point not on the curve, and the identity.
        if peer.len() != P256_POINT || peer[0] != 0x04 {
            return Err(invalid_share());
        }
        let peer = p256::PublicKey::from_sec1_bytes(peer).map_err(|_| invalid_share())?;
        let shared = p256::ecdh::diffie_hellman(self.secret.to_nonzero_scalar(), peer.as_affine());
        Ok(SharedSecret::from(shared.raw_secret_bytes().as_slice()))
    }

    fn pub_key(&self) -> &[u8] {
        &self.public
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::secp256r1
    }
}
