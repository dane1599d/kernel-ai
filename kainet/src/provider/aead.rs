//! TLS 1.3's record protection (RFC 8446 §5.2) with the RustCrypto AEADs:
//! AES-128-GCM, AES-256-GCM and ChaCha20-Poly1305. Each record's nonce is the
//! traffic IV XORed with its sequence number; its additional data is the
//! record header; the inner content type follows the plaintext.

use aes_gcm::aead::consts::{U12, U16};
use aes_gcm::aead::{self, AeadCore, AeadInOut, KeyInit};
use aes_gcm::{Aes128Gcm, Aes256Gcm};
use alloc::boxed::Box;
use chacha20poly1305::ChaCha20Poly1305;
use core::marker::PhantomData;
use rustls::crypto::cipher::{
    AeadKey, InboundOpaqueMessage, InboundPlainMessage, Iv, MessageDecrypter, MessageEncrypter,
    Nonce, OutboundOpaqueMessage, OutboundPlainMessage, PrefixedPayload, Tls13AeadAlgorithm,
    UnsupportedOperationError, make_tls13_aad,
};
use rustls::{ConnectionTrafficSecrets, ContentType, Error, ProtocolVersion};

/// Every AEAD here has a 16-byte tag.
const TAG_LEN: usize = 16;

/// Which traffic-secrets variant an AEAD's keys extract to.
#[derive(Clone, Copy)]
enum Kind {
    Aes128Gcm,
    Aes256Gcm,
    ChaCha20Poly1305,
}

/// A TLS 1.3 AEAD: `A` from the RustCrypto crates.
pub struct Aead<A>(PhantomData<fn() -> A>, Kind);

pub static AES_128_GCM: Aead<Aes128Gcm> = Aead(PhantomData, Kind::Aes128Gcm);
pub static AES_256_GCM: Aead<Aes256Gcm> = Aead(PhantomData, Kind::Aes256Gcm);
pub static CHACHA20_POLY1305: Aead<ChaCha20Poly1305> = Aead(PhantomData, Kind::ChaCha20Poly1305);

/// What every AEAD here offers.
pub trait Cipher: AeadInOut + AeadCore<NonceSize = U12, TagSize = U16> + KeyInit {}

impl<A: AeadInOut + AeadCore<NonceSize = U12, TagSize = U16> + KeyInit> Cipher for A {}

impl<A: Cipher + Send + Sync + 'static> Aead<A> {
    /// The cipher for `key`, which rustls makes exactly `key_len` long.
    fn cipher(key: &AeadKey) -> A {
        A::new_from_slice(key.as_ref()).expect("rustls gives the key length the AEAD asks for")
    }
}

impl<A: Cipher + Send + Sync + 'static> Tls13AeadAlgorithm for Aead<A> {
    fn encrypter(&self, key: AeadKey, iv: Iv) -> Box<dyn MessageEncrypter> {
        Box::new(Encrypter {
            cipher: Self::cipher(&key),
            iv,
        })
    }

    fn decrypter(&self, key: AeadKey, iv: Iv) -> Box<dyn MessageDecrypter> {
        Box::new(Decrypter {
            cipher: Self::cipher(&key),
            iv,
        })
    }

    fn key_len(&self) -> usize {
        A::key_size()
    }

    fn extract_keys(
        &self,
        key: AeadKey,
        iv: Iv,
    ) -> Result<ConnectionTrafficSecrets, UnsupportedOperationError> {
        Ok(match self.1 {
            Kind::Aes128Gcm => ConnectionTrafficSecrets::Aes128Gcm { key, iv },
            Kind::Aes256Gcm => ConnectionTrafficSecrets::Aes256Gcm { key, iv },
            Kind::ChaCha20Poly1305 => ConnectionTrafficSecrets::Chacha20Poly1305 { key, iv },
        })
    }
}

struct Encrypter<A> {
    cipher: A,
    iv: Iv,
}

struct Decrypter<A> {
    cipher: A,
    iv: Iv,
}

fn nonce<A: Cipher>(iv: &Iv, seq: u64) -> aead::Nonce<A> {
    aead::Nonce::<A>::from(Nonce::new(iv, seq).0)
}

impl<A: Cipher + Send + Sync> MessageEncrypter for Encrypter<A> {
    fn encrypt(
        &mut self,
        msg: OutboundPlainMessage<'_>,
        seq: u64,
    ) -> Result<OutboundOpaqueMessage, Error> {
        let total_len = self.encrypted_payload_len(msg.payload.len());
        let mut payload = PrefixedPayload::with_capacity(total_len);
        payload.extend_from_chunks(&msg.payload);
        payload.extend_from_slice(&msg.typ.to_array());
        let aad = make_tls13_aad(total_len);
        let tag = self
            .cipher
            .encrypt_inout_detached(&nonce::<A>(&self.iv, seq), &aad, payload.as_mut().into())
            .map_err(|_| Error::EncryptError)?;
        payload.extend_from_slice(tag.as_slice());
        // Every TLS 1.3 record says TLS 1.2 in its header (RFC 8446 §5.1).
        Ok(OutboundOpaqueMessage::new(
            ContentType::ApplicationData,
            ProtocolVersion::TLSv1_2,
            payload,
        ))
    }

    fn encrypted_payload_len(&self, payload_len: usize) -> usize {
        payload_len + 1 + TAG_LEN
    }
}

impl<A: Cipher + Send + Sync> MessageDecrypter for Decrypter<A> {
    fn decrypt<'a>(
        &mut self,
        mut msg: InboundOpaqueMessage<'a>,
        seq: u64,
    ) -> Result<InboundPlainMessage<'a>, Error> {
        let payload = &mut msg.payload;
        let Some(body_len) = payload.len().checked_sub(TAG_LEN) else {
            return Err(Error::DecryptError);
        };
        let aad = make_tls13_aad(payload.len());
        let (body, tag) = payload.split_at_mut(body_len);
        let tag = aead::Tag::<A>::try_from(&*tag).map_err(|_| Error::DecryptError)?;
        self.cipher
            .decrypt_inout_detached(&nonce::<A>(&self.iv, seq), &aad, body.into(), &tag)
            .map_err(|_| Error::DecryptError)?;
        payload.truncate(body_len);
        msg.into_tls13_unpadded_message()
    }
}
