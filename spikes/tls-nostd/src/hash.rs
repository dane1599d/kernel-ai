//! SHA-256 and SHA-384 and their HMACs, for the TLS 1.3 transcript and key
//! schedule (rustls's `HkdfUsingHmac` builds HKDF from the HMAC).

use alloc::boxed::Box;
use hmac::{Hmac, KeyInit, Mac};
use rustls::crypto::{hash, hmac as rhmac};
use sha2::{Digest, Sha256, Sha384};

pub struct Sha256Hash;
pub struct Sha384Hash;
pub struct Sha256Hmac;
pub struct Sha384Hmac;

struct Context<D>(D);

impl<D: Digest + Clone + Send + Sync + 'static> hash::Context for Context<D> {
    fn fork_finish(&self) -> hash::Output {
        hash::Output::new(&self.0.clone().finalize()[..])
    }

    fn fork(&self) -> Box<dyn hash::Context> {
        Box::new(Context(self.0.clone()))
    }

    fn finish(self: Box<Self>) -> hash::Output {
        hash::Output::new(&self.0.finalize()[..])
    }

    fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }
}

macro_rules! hash_impl {
    ($name:ident, $digest:ty, $alg:ident, $len:literal) => {
        impl hash::Hash for $name {
            fn start(&self) -> Box<dyn hash::Context> {
                Box::new(Context(<$digest>::new()))
            }

            fn hash(&self, data: &[u8]) -> hash::Output {
                hash::Output::new(&<$digest>::digest(data)[..])
            }

            fn output_len(&self) -> usize {
                $len
            }

            fn algorithm(&self) -> hash::HashAlgorithm {
                hash::HashAlgorithm::$alg
            }
        }
    };
}

hash_impl!(Sha256Hash, Sha256, SHA256, 32);
hash_impl!(Sha384Hash, Sha384, SHA384, 48);

struct Key<M>(M);

impl<M: Mac + Clone + Send + Sync> rhmac::Key for Key<M> {
    fn sign_concat(&self, first: &[u8], middle: &[&[u8]], last: &[u8]) -> rhmac::Tag {
        let mut mac = self.0.clone();
        mac.update(first);
        for part in middle {
            mac.update(part);
        }
        mac.update(last);
        rhmac::Tag::new(&mac.finalize().into_bytes()[..])
    }

    fn tag_len(&self) -> usize {
        M::output_size()
    }
}

macro_rules! hmac_impl {
    ($name:ident, $digest:ty, $len:literal) => {
        impl rhmac::Hmac for $name {
            fn with_key(&self, key: &[u8]) -> Box<dyn rhmac::Key> {
                // HMAC takes a key of any length, so this cannot fail.
                Box::new(Key(
                    <Hmac<$digest> as KeyInit>::new_from_slice(key).expect("any key length")
                ))
            }

            fn hash_output_len(&self) -> usize {
                $len
            }
        }
    };
}

hmac_impl!(Sha256Hmac, Sha256, 32);
hmac_impl!(Sha384Hmac, Sha384, 48);
