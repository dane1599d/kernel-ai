//! Kernel AI's TUF client (ADR 0008 §7, ADR 0011 §3): the one trust root for OS
//! images, models and packages, checked in ring 3 by `updated` and `pkg`.
//!
//! It follows the TUF 1.0.37 client workflow (§5) the way python-tuf's ngclient
//! does, so tuf-conformance's suite and python-tuf's repositories are its oracles:
//! - [`canonical`]: OLPC canonical JSON, the bytes every signature covers;
//! - [`keys`]: Ed25519 and ECDSA P-256 keys and their signatures; any other key
//!   type is kept but never verifies, and a role that needs one is refused with
//!   [`Error::UnsupportedKey`];
//! - [`meta`]: root, timestamp, snapshot and targets metadata, parsed strictly;
//! - [`trusted`]: the trusted set, which takes each new role only after its
//!   signatures, version, hashes and expiry hold;
//! - [`updater`]: the refresh (root rotation, then timestamp, snapshot and
//!   targets) and the preorder depth-first search of delegations for a target,
//!   over a [`updater::Remote`] and a [`updater::Cache`].
//!
//! Time is the caller's: seconds since 1970 UTC, fixed for one refresh. This crate
//! is `no_std` + `alloc`.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod canonical;
pub mod keys;
pub mod meta;
pub mod trusted;
pub mod updater;

use alloc::string::String;

/// Why metadata or a target was refused, in the client's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not metadata of the expected shape.
    Malformed(String),
    /// A role needs signatures from keys of a type this client does not check
    /// (it checks Ed25519 and ECDSA P-256 only).
    UnsupportedKey(String),
    /// Fewer valid signatures than the role's threshold.
    Threshold(String),
    /// Its `expires` is not after the time of the refresh.
    Expired(String),
    /// A version other than the one named, or older than the trusted one.
    Version(String),
    /// A length or hash other than the one named.
    Hash(String),
    /// Over its role's or its target's size limit.
    TooLarge(String),
    /// The repository has no such file.
    NotFound(String),
    /// The transport or the cache failed.
    Io(String),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let (kind, why) = match self {
            Error::Malformed(w) => ("malformed metadata", w),
            Error::UnsupportedKey(w) => ("unsupported key type", w),
            Error::Threshold(w) => ("below the signature threshold", w),
            Error::Expired(w) => ("expired", w),
            Error::Version(w) => ("bad version", w),
            Error::Hash(w) => ("length or hash mismatch", w),
            Error::TooLarge(w) => ("too large", w),
            Error::NotFound(w) => ("not found", w),
            Error::Io(w) => ("i/o", w),
        };
        write!(f, "{kind}: {why}")
    }
}

/// `Error::Malformed` with `why`.
pub(crate) fn malformed<T>(why: impl Into<String>) -> Result<T, Error> {
    Err(Error::Malformed(why.into()))
}

/// `name` percent-encoded as python's `urllib.parse.quote(name, "")` does, for
/// role names in URLs and file names: everything but letters, digits and
/// `_.-~` becomes `%XX`.
pub fn quote(name: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-' | b'~') {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 15)]));
        }
    }
    out
}

/// Lower-case hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|b| [HEX[usize::from(b >> 4)], HEX[usize::from(b & 15)]])
        .map(char::from)
        .collect()
}

/// Hex of either case as bytes; None if it is not hex.
pub fn unhex(text: &str) -> Option<alloc::vec::Vec<u8>> {
    let digit = |c: u8| char::from(c).to_digit(16).map(|d| d as u8);
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .chunks(2)
        .map(|pair| Some(digit(pair[0])? << 4 | digit(pair[1])?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_matches_python() {
        assert_eq!(quote("targets"), "targets");
        assert_eq!(quote("registry.npmjs.org"), "registry.npmjs.org");
        assert_eq!(quote("../delegatedrole"), "..%2Fdelegatedrole");
        assert_eq!(quote("?"), "%3F");
        assert_eq!(quote("#"), "%23");
        assert_eq!(quote("a b~é"), "a%20b~%C3%A9");
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(hex(&[0, 0xab, 0xff]), "00abff");
        assert_eq!(unhex("00ABff"), Some(alloc::vec![0, 0xab, 0xff]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
        assert_eq!(unhex(""), Some(alloc::vec![]));
    }
}
