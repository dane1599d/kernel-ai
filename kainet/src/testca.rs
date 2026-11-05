//! A test certificate authority (feature `testca`; host only, never in the
//! OS): the host tests, `tlsbench`'s fixtures and xtask's HTTPS mock make
//! their CA and server certificates here. Keys come from seeds, so a fixture
//! can be made again byte for byte.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use p256::ecdsa::signature::Signer;
use p256::pkcs8::EncodePrivateKey;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyUsagePurpose, PublicKeyData, SerialNumber, SignatureAlgorithm, SigningKey,
};
use sha2::Digest;

/// The key types a test CA or server may have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    P256,
    P384,
    Ed25519,
}

/// A key made from a seed.
pub struct Key {
    kind: KeyKind,
    seed: [u8; 32],
    public: Vec<u8>,
}

impl Key {
    /// The key of `kind` for `seed`. A P-256 or P-384 seed that is not a
    /// scalar below the group order is refused; any other seed is one.
    pub fn new(kind: KeyKind, seed: [u8; 32]) -> Result<Self, String> {
        let public = match kind {
            KeyKind::P256 => p256::SecretKey::from_slice(&seed)
                .map_err(|_| String::from("not a P-256 scalar"))?
                .public_key()
                .to_sec1_bytes()
                .into_vec(),
            KeyKind::P384 => {
                let mut wide = [0u8; 48];
                wide[16..].copy_from_slice(&seed);
                p384::SecretKey::from_slice(&wide)
                    .map_err(|_| String::from("not a P-384 scalar"))?
                    .public_key()
                    .to_sec1_bytes()
                    .into_vec()
            }
            KeyKind::Ed25519 => ed25519_dalek::SigningKey::from_bytes(&seed)
                .verifying_key()
                .to_bytes()
                .to_vec(),
        };
        Ok(Self { kind, seed, public })
    }

    /// The private key in PKCS#8 DER, for a P-256 server key.
    pub fn pkcs8(&self) -> Result<Vec<u8>, String> {
        match self.kind {
            KeyKind::P256 => {
                let key =
                    p256::SecretKey::from_slice(&self.seed).map_err(|e| alloc::format!("{e}"))?;
                Ok(key
                    .to_pkcs8_der()
                    .map_err(|e| alloc::format!("{e}"))?
                    .as_bytes()
                    .to_vec())
            }
            _ => Err(String::from("only a P-256 key serves")),
        }
    }
}

impl PublicKeyData for Key {
    fn der_bytes(&self) -> &[u8] {
        &self.public
    }

    fn algorithm(&self) -> &'static SignatureAlgorithm {
        match self.kind {
            KeyKind::P256 => &rcgen::PKCS_ECDSA_P256_SHA256,
            KeyKind::P384 => &rcgen::PKCS_ECDSA_P384_SHA384,
            KeyKind::Ed25519 => &rcgen::PKCS_ED25519,
        }
    }
}

impl SigningKey for Key {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, rcgen::Error> {
        Ok(match self.kind {
            KeyKind::P256 => {
                let key = p256::ecdsa::SigningKey::from_slice(&self.seed)
                    .map_err(|_| rcgen::Error::RemoteKeyError)?;
                let signature: p256::ecdsa::Signature = key.sign(msg);
                signature.to_der().as_bytes().to_vec()
            }
            KeyKind::P384 => {
                let mut wide = [0u8; 48];
                wide[16..].copy_from_slice(&self.seed);
                let key = p384::ecdsa::SigningKey::from_slice(&wide)
                    .map_err(|_| rcgen::Error::RemoteKeyError)?;
                let signature: p384::ecdsa::Signature = key.sign(msg);
                signature.to_der().as_bytes().to_vec()
            }
            KeyKind::Ed25519 => {
                let key = ed25519_dalek::SigningKey::from_bytes(&self.seed);
                key.sign(msg).to_bytes().to_vec()
            }
        })
    }
}

/// A validity period, as (year, month, day) pairs.
pub type Days = ((i32, u8, u8), (i32, u8, u8));

/// A serial number from what the certificate holds, so a fixture is made
/// again byte for byte: 16 bytes of SHA-256, its top bit clear so the DER
/// integer stays positive.
fn serial(key: &Key, names: &[&str]) -> SerialNumber {
    let mut hash = sha2::Sha256::new();
    hash.update(key.der_bytes());
    for name in names {
        hash.update(name.as_bytes());
        hash.update([0]);
    }
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash.finalize()[..16]);
    bytes[0] &= 0x7f;
    SerialNumber::from_slice(&bytes)
}

fn dn(name: &str) -> DistinguishedName {
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, name);
    dn
}

/// A certificate authority whose certificate signs itself.
pub struct Ca {
    issuer: Issuer<'static, Key>,
    der: Vec<u8>,
}

impl Ca {
    pub fn new(name: &str, key: Key, valid: Days) -> Result<Self, rcgen::Error> {
        let mut params = CertificateParams::default();
        params.distinguished_name = dn(name);
        params.serial_number = Some(serial(&key, &[name]));
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params.not_before = rcgen::date_time_ymd(valid.0.0, valid.0.1, valid.0.2);
        params.not_after = rcgen::date_time_ymd(valid.1.0, valid.1.1, valid.1.2);
        let der = params.self_signed(&key)?.der().to_vec();
        Ok(Self {
            issuer: Issuer::new(params, key),
            der,
        })
    }

    /// The CA's certificate in DER: the root a client trusts.
    pub fn der(&self) -> &[u8] {
        &self.der
    }

    /// A server certificate for `names`, valid for `valid`, holding `key`.
    pub fn issue(&self, names: &[&str], key: &Key, valid: Days) -> Result<Vec<u8>, rcgen::Error> {
        let owned: Vec<String> = names.iter().map(|n| String::from(*n)).collect();
        let mut params = CertificateParams::new(owned)?;
        params.distinguished_name = dn("kainet test server");
        params.serial_number = Some(serial(key, names));
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.not_before = rcgen::date_time_ymd(valid.0.0, valid.0.1, valid.0.2);
        params.not_after = rcgen::date_time_ymd(valid.1.0, valid.1.1, valid.1.2);
        Ok(params.signed_by(key, &self.issuer)?.der().to_vec())
    }
}
