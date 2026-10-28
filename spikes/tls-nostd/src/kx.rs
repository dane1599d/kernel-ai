//! Key exchange: X25519, secp256r1 and secp384r1, each an ephemeral key from the
//! provider's random source. A peer share that is not a valid point, or that
//! gives X25519's all-zero secret, is refused.

use crate::random;
use alloc::boxed::Box;
use alloc::vec::Vec;
use rustls::crypto::{ActiveKeyExchange, SharedSecret, SupportedKxGroup};
use rustls::{Error, NamedGroup, PeerMisbehaved};

#[derive(Debug)]
pub struct X25519;
#[derive(Debug)]
pub struct P256;
#[derive(Debug)]
pub struct P384;

fn bad_share() -> Error {
    PeerMisbehaved::InvalidKeyShare.into()
}

struct X25519Active {
    secret: x25519_dalek::StaticSecret,
    public: [u8; 32],
}

impl SupportedKxGroup for X25519 {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        let mut seed = [0u8; 32];
        random::fill(&mut seed)?;
        let secret = x25519_dalek::StaticSecret::from(seed);
        let public = x25519_dalek::PublicKey::from(&secret).to_bytes();
        Ok(Box::new(X25519Active { secret, public }))
    }

    fn name(&self) -> NamedGroup {
        NamedGroup::X25519
    }
}

impl ActiveKeyExchange for X25519Active {
    fn complete(self: Box<Self>, peer: &[u8]) -> Result<SharedSecret, Error> {
        let peer: [u8; 32] = peer.try_into().map_err(|_| bad_share())?;
        let shared = self
            .secret
            .diffie_hellman(&x25519_dalek::PublicKey::from(peer));
        if !shared.was_contributory() {
            return Err(bad_share());
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

/// A NIST curve's ephemeral key: its scalar and its uncompressed public point.
macro_rules! nist_group {
    ($group:ident, $active:ident, $curve:ident, $named:ident, $len:literal) => {
        struct $active {
            secret: $curve::SecretKey,
            public: Vec<u8>,
        }

        impl SupportedKxGroup for $group {
            fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
                // A random field element is a valid scalar with overwhelming
                // probability; draw again on the rare zero or overflow.
                loop {
                    let mut bytes = [0u8; $len];
                    random::fill(&mut bytes)?;
                    if let Ok(secret) = $curve::SecretKey::from_slice(&bytes) {
                        // NIST curves encode uncompressed (SEC 1 §2.3.3).
                        let public = secret.public_key().to_sec1_bytes().into_vec();
                        return Ok(Box::new($active { secret, public }));
                    }
                }
            }

            fn name(&self) -> NamedGroup {
                NamedGroup::$named
            }
        }

        impl ActiveKeyExchange for $active {
            fn complete(self: Box<Self>, peer: &[u8]) -> Result<SharedSecret, Error> {
                // RFC 8446 §4.2.8.2: only the uncompressed form.
                if peer.first() != Some(&4) {
                    return Err(bad_share());
                }
                let peer = $curve::PublicKey::from_sec1_bytes(peer).map_err(|_| bad_share())?;
                let shared =
                    $curve::ecdh::diffie_hellman(self.secret.to_nonzero_scalar(), peer.as_affine());
                Ok(SharedSecret::from(&shared.raw_secret_bytes()[..]))
            }

            fn pub_key(&self) -> &[u8] {
                &self.public
            }

            fn group(&self) -> NamedGroup {
                NamedGroup::$named
            }
        }
    };
}

nist_group!(P256, P256Active, p256, secp256r1, 32);
nist_group!(P384, P384Active, p384, secp384r1, 48);
