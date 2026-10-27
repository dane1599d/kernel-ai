//! Keys as TUF metadata lists them, and their signatures.
//!
//! - `ed25519`/`ed25519`: a 32-byte public key in hex; 64-byte signatures in hex.
//! - `ecdsa` (or the older `ecdsa-sha2-nistp256` keytype) with scheme
//!   `ecdsa-sha2-nistp256`: the public key as a PEM SubjectPublicKeyInfo, as
//!   securesystemslib and tuf-on-ci write it, or as 65 bytes of uncompressed
//!   SEC1 point in hex, as early sigstore roots do; signatures are DER in hex,
//!   over SHA-256 of the message. YubiKeys and cloud KMS keys sign this way.
//!
//! Any other type or scheme is kept as [`Key::Unsupported`]: metadata naming it
//! still parses, but its signatures never count.

use crate::unhex;
use alloc::string::String;
use alloc::vec::Vec;
use p256::ecdsa::signature::Verifier;
use serde_json::Value;

/// A key, as its metadata's `keys` entry gives it.
#[derive(Clone, Debug)]
pub enum Key {
    Ed25519(ed25519_compact::PublicKey),
    P256(p256::ecdsa::VerifyingKey),
    /// A type or scheme this client does not check: `<keytype>/<scheme>`.
    Unsupported(String),
    /// A supported type whose public key does not decode; it never verifies.
    Invalid,
}

/// The DER prefix of a P-256 SubjectPublicKeyInfo, before its 65-byte point:
/// SEQUENCE { SEQUENCE { id-ecPublicKey, prime256v1 }, BIT STRING }.
const P256_SPKI: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

impl Key {
    /// The key a `keys` entry describes. Only a shape that is not a key at all
    /// (no `keytype`, `scheme` or `keyval.public` string) is an error.
    pub fn parse(entry: &Value) -> Result<Self, &'static str> {
        let (Some(keytype), Some(scheme), Some(public)) = (
            entry["keytype"].as_str(),
            entry["scheme"].as_str(),
            entry["keyval"]["public"].as_str(),
        ) else {
            return Err("a key is not keytype, scheme and keyval");
        };
        Ok(match (keytype, scheme) {
            ("ed25519", "ed25519") => unhex(public)
                .and_then(|raw| ed25519_compact::PublicKey::from_slice(&raw).ok())
                .map_or(Key::Invalid, Key::Ed25519),
            ("ecdsa" | "ecdsa-sha2-nistp256", "ecdsa-sha2-nistp256") => {
                let point = match public.starts_with("-----BEGIN PUBLIC KEY-----") {
                    true => pem_spki(public),
                    false => unhex(public),
                };
                point
                    .and_then(|p| p256::ecdsa::VerifyingKey::from_sec1_bytes(&p).ok())
                    .map_or(Key::Invalid, Key::P256)
            }
            _ => Key::Unsupported(alloc::format!("{keytype}/{scheme}")),
        })
    }

    /// Whether `sig` (hex, as the metadata holds it) is this key's signature of
    /// `message`.
    pub fn verifies(&self, message: &[u8], sig: &str) -> bool {
        let Some(raw) = unhex(sig) else {
            return false;
        };
        match self {
            Key::Ed25519(key) => ed25519_compact::Signature::from_slice(&raw)
                .is_ok_and(|sig| key.verify(message, &sig).is_ok()),
            Key::P256(key) => der_signature(&raw)
                .and_then(|rs| p256::ecdsa::Signature::from_slice(&rs).ok())
                // Python's cryptography signs with either s; s and n - s verify alike.
                .map(|sig| sig.normalize_s())
                .is_some_and(|sig| key.verify(message, &sig).is_ok()),
            Key::Unsupported(_) | Key::Invalid => false,
        }
    }
}

/// The SEC1 point of a PEM `PUBLIC KEY` holding a P-256 SubjectPublicKeyInfo.
fn pem_spki(pem: &str) -> Option<Vec<u8>> {
    let body = pem
        .trim()
        .strip_prefix("-----BEGIN PUBLIC KEY-----")?
        .strip_suffix("-----END PUBLIC KEY-----")?;
    let der = base64(body)?;
    der.strip_prefix(&P256_SPKI[..])
        .filter(|point| point.len() == 65)
        .map(<[u8]>::to_vec)
}

/// Standard base64 with padding, whitespace between characters ignored.
fn base64(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let chars: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if chars.is_empty() || !chars.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(chars.len() / 4 * 3);
    let last = chars.len() / 4 - 1;
    for (i, quad) in chars.chunks(4).enumerate() {
        let pad = quad.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && i != last) {
            return None;
        }
        let mut n = 0u32;
        for &c in &quad[..4 - pad] {
            n = n << 6 | value(c)?;
        }
        n <<= 6 * pad as u32;
        out.extend_from_slice(&n.to_be_bytes()[1..4 - pad]);
    }
    Some(out)
}

/// `r || s`, 32 bytes each, of a DER ECDSA signature:
/// SEQUENCE { INTEGER r, INTEGER s }, each a minimal positive integer.
fn der_signature(der: &[u8]) -> Option<[u8; 64]> {
    let (&[0x30, len], rest) = der.split_first_chunk::<2>()? else {
        return None;
    };
    if usize::from(len) != rest.len() {
        return None;
    }
    let mut out = [0u8; 64];
    let mut rest = rest;
    for half in out.chunks_mut(32) {
        let (&[0x02, n], tail) = rest.split_first_chunk::<2>()? else {
            return None;
        };
        let n = usize::from(n);
        let int = tail.get(..n)?;
        // Minimal: no leading zero unless the next byte's top bit is set, and
        // never negative.
        let minimal = match int {
            [] => false,
            [0, next, ..] => next & 0x80 != 0,
            [first, ..] => first & 0x80 == 0,
        };
        let value = int.strip_prefix(&[0]).unwrap_or(int);
        if !minimal || value.len() > 32 {
            return None;
        }
        half[32 - value.len()..].copy_from_slice(value);
        rest = &tail[n..];
    }
    rest.is_empty().then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use serde_json::json;

    #[test]
    fn base64_decodes_with_padding_and_line_breaks() {
        assert_eq!(base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64("aGVs\nbG8h").unwrap(), b"hello!");
        assert_eq!(base64("aGU=").unwrap(), b"he");
        for bad in ["", "aGVsbG8", "aG=s", "aGVs=G8=", "a===", "aGV$"] {
            assert_eq!(base64(bad), None, "{bad}");
        }
    }

    #[test]
    fn der_signatures_parse_strictly() {
        let mut der = vec![0x30, 0x45, 0x02, 0x21, 0x00, 0x80];
        der.extend([1; 31]);
        der.extend([0x02, 0x20, 0x7f]);
        der.extend([2; 31]);
        let rs = der_signature(&der).unwrap();
        assert_eq!(rs[0], 0x80);
        assert_eq!(rs[32], 0x7f);
        // A short r is padded on the left.
        let short = [0x30, 0x06, 0x02, 0x01, 0x05, 0x02, 0x01, 0x06];
        let rs = der_signature(&short).unwrap();
        assert_eq!((rs[31], rs[63]), (5, 6));
        for bad in [
            &[0x30, 0x06, 0x02, 0x01, 0x05, 0x02, 0x01][..],
            &[0x30, 0x07, 0x02, 0x02, 0x00, 0x05, 0x02, 0x01, 0x06],
            &[0x30, 0x06, 0x02, 0x01, 0x85, 0x02, 0x01, 0x06],
            &[0x30, 0x05, 0x02, 0x00, 0x02, 0x01, 0x06],
            &[0x31, 0x06, 0x02, 0x01, 0x05, 0x02, 0x01, 0x06],
            &[0x30, 0x07, 0x02, 0x01, 0x05, 0x02, 0x01, 0x06, 0x00],
        ] {
            assert_eq!(der_signature(bad), None, "{bad:02x?}");
        }
    }

    #[test]
    fn other_key_types_are_kept_as_unsupported() {
        let key = |t: &str, s: &str, p: &str| {
            Key::parse(&json!({ "keytype": t, "scheme": s, "keyval": { "public": p } })).unwrap()
        };
        assert!(
            matches!(key("rsa", "rsassa-pss-sha256", "x"), Key::Unsupported(t) if t == "rsa/rsassa-pss-sha256")
        );
        assert!(matches!(
            key("ecdsa", "ecdsa-sha2-nistp384", "x"),
            Key::Unsupported(_)
        ));
        assert!(matches!(key("ed25519", "ed25519", "00"), Key::Invalid));
        assert!(Key::parse(&json!({ "keytype": "ed25519" })).is_err());
    }
}
