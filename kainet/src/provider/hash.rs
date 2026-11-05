//! SHA-256 and SHA-384 for the handshake transcript, and HMAC over them for
//! TLS 1.3's key schedule (rustls's `HkdfUsingHmac`).

use alloc::boxed::Box;
use core::marker::PhantomData;
use rustls::crypto::hash::{self, HashAlgorithm};
use rustls::crypto::hmac;
use sha2::Digest;

/// A hash for rustls: `D` from the RustCrypto `sha2` crate.
pub struct Hash<D>(PhantomData<fn() -> D>, HashAlgorithm);

pub static SHA256: Hash<sha2::Sha256> = Hash(PhantomData, HashAlgorithm::SHA256);
pub static SHA384: Hash<sha2::Sha384> = Hash(PhantomData, HashAlgorithm::SHA384);

impl<D: Digest + Clone + Send + Sync + 'static> hash::Hash for Hash<D> {
    fn start(&self) -> Box<dyn hash::Context> {
        Box::new(Context(D::new()))
    }

    fn hash(&self, data: &[u8]) -> hash::Output {
        hash::Output::new(D::digest(data).as_slice())
    }

    fn output_len(&self) -> usize {
        <D as Digest>::output_size()
    }

    fn algorithm(&self) -> HashAlgorithm {
        self.1
    }
}

struct Context<D>(D);

impl<D: Digest + Clone + Send + Sync + 'static> hash::Context for Context<D> {
    fn fork_finish(&self) -> hash::Output {
        hash::Output::new(self.0.clone().finalize().as_slice())
    }

    fn fork(&self) -> Box<dyn hash::Context> {
        Box::new(Context(self.0.clone()))
    }

    fn finish(self: Box<Self>) -> hash::Output {
        hash::Output::new(self.0.finalize().as_slice())
    }

    fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }
}

/// HMAC for rustls: `M` is `hmac::Hmac` over one of the hashes above.
pub struct Hmac<M>(PhantomData<fn() -> M>);

pub static HMAC_SHA256: Hmac<::hmac::Hmac<sha2::Sha256>> = Hmac(PhantomData);
pub static HMAC_SHA384: Hmac<::hmac::Hmac<sha2::Sha384>> = Hmac(PhantomData);

impl<M> hmac::Hmac for Hmac<M>
where
    M: ::hmac::Mac + ::hmac::KeyInit + Clone + Send + Sync + 'static,
{
    fn with_key(&self, key: &[u8]) -> Box<dyn hmac::Key> {
        // HMAC takes a key of any length: a longer one is hashed, a shorter
        // one padded (RFC 2104), so this never fails.
        let mac = <M as ::hmac::KeyInit>::new_from_slice(key).expect("HMAC takes any key");
        Box::new(Key(mac))
    }

    fn hash_output_len(&self) -> usize {
        <M as ::hmac::digest::OutputSizeUser>::output_size()
    }
}

struct Key<M>(M);

impl<M: ::hmac::Mac + Clone + Send + Sync> hmac::Key for Key<M> {
    fn sign_concat(&self, first: &[u8], middle: &[&[u8]], last: &[u8]) -> hmac::Tag {
        let mut mac = self.0.clone();
        mac.update(first);
        for part in middle {
            mac.update(part);
        }
        mac.update(last);
        hmac::Tag::new(mac.finalize().into_bytes().as_slice())
    }

    fn tag_len(&self) -> usize {
        <M as ::hmac::digest::OutputSizeUser>::output_size()
    }
}
