//! The in-memory test server's key: ECDSA P-256 with SHA-256, deterministic
//! (RFC 6979), so it needs no random source. Only the bench and the tests sign;
//! the client never does.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use rustls::sign::{Signer, SigningKey};
use rustls::{Error, SignatureAlgorithm, SignatureScheme};

#[derive(Debug, Clone)]
pub struct P256Key(p256::ecdsa::SigningKey);

impl P256Key {
    /// A key from its 32-byte scalar.
    pub fn from_scalar(scalar: &[u8]) -> Option<Arc<dyn SigningKey>> {
        let key = p256::ecdsa::SigningKey::from_slice(scalar).ok()?;
        Some(Arc::new(Self(key)))
    }
}

impl SigningKey for P256Key {
    fn choose_scheme(&self, offered: &[SignatureScheme]) -> Option<Box<dyn Signer>> {
        offered
            .contains(&SignatureScheme::ECDSA_NISTP256_SHA256)
            .then(|| Box::new(self.clone()) as Box<dyn Signer>)
    }

    fn algorithm(&self) -> SignatureAlgorithm {
        SignatureAlgorithm::ECDSA
    }
}

impl Signer for P256Key {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, Error> {
        use ecdsa::signature::Signer as _;
        let sig: p256::ecdsa::Signature = self.0.sign(message);
        Ok(sig.to_der().as_bytes().to_vec())
    }

    fn scheme(&self) -> SignatureScheme {
        SignatureScheme::ECDSA_NISTP256_SHA256
    }
}
