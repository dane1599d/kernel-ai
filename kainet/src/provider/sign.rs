//! Private keys. The OS's clients never sign, so without the `server`
//! feature every key is refused. With it, a test server (xtask's HTTPS mock,
//! the host tests, `tlsbench`'s loopback) signs with a P-256 key in PKCS#8,
//! deterministically (RFC 6979), so it needs no random numbers.

use alloc::sync::Arc;
use pki_types::PrivateKeyDer;
use rustls::Error;
use rustls::crypto::KeyProvider;
use rustls::sign::SigningKey;

#[derive(Debug)]
pub struct Keys;

impl KeyProvider for Keys {
    fn load_private_key(
        &self,
        key_der: PrivateKeyDer<'static>,
    ) -> Result<Arc<dyn SigningKey>, Error> {
        #[cfg(feature = "server")]
        if let PrivateKeyDer::Pkcs8(der) = &key_der {
            use p256::pkcs8::DecodePrivateKey;
            if let Ok(key) = p256::ecdsa::SigningKey::from_pkcs8_der(der.secret_pkcs8_der()) {
                return Ok(Arc::new(server::EcdsaP256(key)));
            }
        }
        let _ = key_der;
        Err(Error::General(alloc::string::String::from(
            "kainet signs only with a P-256 key in PKCS#8, and only as a test server",
        )))
    }
}

#[cfg(feature = "server")]
mod server {
    use alloc::boxed::Box;
    use alloc::vec::Vec;
    use p256::ecdsa::signature::Signer as _;
    use rustls::sign::{Signer, SigningKey};
    use rustls::{Error, SignatureAlgorithm, SignatureScheme};

    #[derive(Debug)]
    pub struct EcdsaP256(pub p256::ecdsa::SigningKey);

    impl SigningKey for EcdsaP256 {
        fn choose_scheme(&self, offered: &[SignatureScheme]) -> Option<Box<dyn Signer>> {
            offered
                .contains(&SignatureScheme::ECDSA_NISTP256_SHA256)
                .then(|| Box::new(EcdsaP256Signer(self.0.clone())) as Box<dyn Signer>)
        }

        fn algorithm(&self) -> SignatureAlgorithm {
            SignatureAlgorithm::ECDSA
        }
    }

    #[derive(Debug)]
    struct EcdsaP256Signer(p256::ecdsa::SigningKey);

    impl Signer for EcdsaP256Signer {
        fn sign(&self, message: &[u8]) -> Result<Vec<u8>, Error> {
            let signature: p256::ecdsa::Signature = self.0.sign(message);
            Ok(signature.to_der().as_bytes().to_vec())
        }

        fn scheme(&self) -> SignatureScheme {
            SignatureScheme::ECDSA_NISTP256_SHA256
        }
    }
}
