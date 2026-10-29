//! Kernel AI's packages (ADR 0011): the `.kpkg` file, its header, versions,
//! resolution, and the store of objects and numbered generations that `pkg`
//! switches between.
//!
//! - [`bundle`]: `kai-pkg 1`, length-framed files with plain relative paths;
//! - [`catalog`]: the Apps sheet's rows, installed then offered, in
//!   `kai-catalog 1`;
//! - [`header`]: `kai-package 1`, the package's first file, parsed strictly;
//! - [`version`]: SemVer versions and the `^`, `~`, `=` and comparison ranges;
//! - [`resolve`]: no solver (§6): the newest compatible version that is not
//!   yanked, keeping an installed version that already satisfies;
//! - [`generation`]: `kai-gen 1`, one numbered list of installed packages;
//! - [`image`]: the image's signed repository, checked with `kaituf`, as
//!   generation 1's packages;
//! - [`index`]: a package's `custom.kai` index entry, and cargo's rule against
//!   its header;
//! - [`mirror`]: installs from the fixed `pkgrepo=` mirror, its metadata checked
//!   with `kaituf`, its packages fetched in pieces;
//! - [`memvol`]: the store's volume in memory;
//! - [`fsvol`]: the store's [`store::Volume`] over `kai-fs 1`, as `pkg` holds
//!   `/system/pkg`;
//! - [`store`]: `/system/pkg`'s objects, generations, `current`, `good`, failed
//!   hashes and GC (§5, §7), over a [`store::Volume`]. A switch writes the new
//!   generation, `current`, its grants and its log line in one transaction (S11-2).
//!
//! This crate is `no_std` + `alloc`: the `pkg` service and the host tests run the
//! same code.

#![no_std]

extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod bundle;
pub mod catalog;
pub mod fsvol;
pub mod generation;
pub mod header;
pub mod image;
pub mod index;
pub mod memvol;
pub mod mirror;
pub mod resolve;
pub mod store;
pub mod version;

use alloc::string::String;
use sha2::{Digest, Sha256};
use version::Version;

/// The OS's virtual packages (§6): the `kai:*` WIT packages it provides, which
/// an app's `world` depends on.
pub const PROVIDED: &[(&str, Version)] = &[("kai:app", Version::new(0, 1, 0))];

/// An object's name: its SHA-256.
pub type Sha = [u8; 32];

/// `bytes`' SHA-256.
pub fn sha256(bytes: &[u8]) -> Sha {
    Sha256::digest(bytes).into()
}

/// `sha` as 64 lower-case hex digits, the form paths and generations use.
pub fn hex(sha: &Sha) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    sha.iter()
        .flat_map(|b| [DIGITS[usize::from(b >> 4)], DIGITS[usize::from(b & 15)]])
        .map(char::from)
        .collect()
}

/// 64 lower-case hex digits as a SHA-256; anything else is None.
pub fn unhex(text: &str) -> Option<Sha> {
    let digit = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let bytes = text.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut sha = [0u8; 32];
    for (i, [high, low]) in bytes.as_chunks::<2>().0.iter().enumerate() {
        sha[i] = digit(*high)? << 4 | digit(*low)?;
    }
    Some(sha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_refuses_upper_case() {
        let sha = sha256(b"notes");
        assert_eq!(unhex(&hex(&sha)), Some(sha));
        assert_eq!(unhex(&hex(&sha).to_uppercase()), None);
        assert_eq!(unhex("ab"), None);
    }
}
