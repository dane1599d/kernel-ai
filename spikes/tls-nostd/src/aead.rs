//! TLS 1.3's three AEADs: AES-128-GCM, AES-256-GCM and ChaCha20-Poly1305, each
//! sealing a record in place with the inner content type appended (RFC 8446
//! §5.2) and opening it in place in the caller's receive buffer.

use aes_gcm::aead::{AeadInOut, KeyInit};
use aes_gcm::{Aes128Gcm, Aes256Gcm};
use alloc::boxed::Box;
use chacha20poly1305::ChaCha20Poly1305;
use rustls::crypto::cipher::{
    AeadKey, InboundOpaqueMessage, InboundPlainMessage, Iv, MessageDecrypter, MessageEncrypter,
    Nonce, OutboundOpaqueMessage, OutboundPlainMessage, PrefixedPayload, Tls13AeadAlgorithm,
    UnsupportedOperationError, make_tls13_aad,
};
use rustls::{ConnectionTrafficSecrets, ContentType, Error, ProtocolVersion};

const TAG_LEN: usize = 16;

pub struct Aes128;
pub struct Aes256;
pub struct ChaCha;

/// One direction's cipher and IV; the record sequence number makes the nonce.
struct Cipher<C> {
    cipher: C,
    iv: Iv,
}

impl<C: AeadInOut<NonceSize = aes_gcm::aead::consts::U12> + Send + Sync> MessageEncrypter
    for Cipher<C>
{
    fn encrypt(
        &mut self,
        msg: OutboundPlainMessage<'_>,
        seq: u64,
    ) -> Result<OutboundOpaqueMessage, Error> {
        let total = self.encrypted_payload_len(msg.payload.len());
        let mut payload = PrefixedPayload::with_capacity(total);
        payload.extend_from_chunks(&msg.payload);
        payload.extend_from_slice(&msg.typ.to_array());
        let nonce = Nonce::new(&self.iv, seq).0;
        let aad = make_tls13_aad(total);
        let tag = self
            .cipher
            .encrypt_inout_detached(&nonce.into(), &aad, payload.as_mut().into())
            .map_err(|_| Error::EncryptError)?;
        payload.extend_from_slice(&tag);
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

impl<C: AeadInOut<NonceSize = aes_gcm::aead::consts::U12> + Send + Sync> MessageDecrypter
    for Cipher<C>
{
    fn decrypt<'a>(
        &mut self,
        mut msg: InboundOpaqueMessage<'a>,
        seq: u64,
    ) -> Result<InboundPlainMessage<'a>, Error> {
        let payload = &mut msg.payload;
        let aad = make_tls13_aad(payload.len());
        let body = payload
            .len()
            .checked_sub(TAG_LEN)
            .ok_or(Error::DecryptError)?;
        let nonce = Nonce::new(&self.iv, seq).0;
        let (data, tag) = payload.split_at_mut(body);
        let tag = (&*tag).try_into().map_err(|_| Error::DecryptError)?;
        self.cipher
            .decrypt_inout_detached(&nonce.into(), &aad, data.into(), tag)
            .map_err(|_| Error::DecryptError)?;
        payload.truncate(body);
        msg.into_tls13_unpadded_message()
    }
}

macro_rules! aead_impl {
    ($name:ident, $cipher:ty, $len:literal, $secrets:ident) => {
        impl Tls13AeadAlgorithm for $name {
            fn encrypter(&self, key: AeadKey, iv: Iv) -> Box<dyn MessageEncrypter> {
                Box::new(Cipher {
                    cipher: <$cipher>::new_from_slice(key.as_ref()).expect("key length"),
                    iv,
                })
            }

            fn decrypter(&self, key: AeadKey, iv: Iv) -> Box<dyn MessageDecrypter> {
                Box::new(Cipher {
                    cipher: <$cipher>::new_from_slice(key.as_ref()).expect("key length"),
                    iv,
                })
            }

            fn key_len(&self) -> usize {
                $len
            }

            fn extract_keys(
                &self,
                key: AeadKey,
                iv: Iv,
            ) -> Result<ConnectionTrafficSecrets, UnsupportedOperationError> {
                Ok(ConnectionTrafficSecrets::$secrets { key, iv })
            }
        }
    };
}

aead_impl!(Aes128, Aes128Gcm, 16, Aes128Gcm);
aead_impl!(Aes256, Aes256Gcm, 32, Aes256Gcm);
aead_impl!(ChaCha, ChaCha20Poly1305, 32, Chacha20Poly1305);
