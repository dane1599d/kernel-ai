//! The test repository: six packages with dependencies and grants, each object a
//! `kpkg 1` header and deterministic bytes. Objects are small (1-12 KiB) because
//! the switch, not the transfer, is what this spike measures; each is still
//! fetched in more than one piece, so partial downloads exist.

use sha2::{Digest, Sha256};

#[derive(Debug, PartialEq, Eq)]
pub struct Package {
    pub name: &'static str,
    pub version: &'static str,
    pub depends: &'static [&'static str],
    /// What ADR 0001 makes the user grant; they land in `kai-grants 1`.
    pub grants: &'static [&'static str],
    pub size: usize,
}

pub const REPOSITORY: &[Package] = &[
    Package {
        name: "notes",
        version: "2.1.0",
        depends: &["spell-en", "ui-kit"],
        grants: &[],
        size: 9 << 10,
    },
    Package {
        name: "spell-en",
        version: "1.2.3",
        depends: &[],
        grants: &[],
        size: 12 << 10,
    },
    Package {
        name: "ui-kit",
        version: "0.1.0",
        depends: &[],
        grants: &[],
        size: 1 << 10,
    },
    Package {
        name: "weather",
        version: "0.1.0",
        depends: &["ui-kit"],
        grants: &["network"],
        size: 5 << 10,
    },
    Package {
        name: "camera",
        version: "0.2.0",
        depends: &[],
        grants: &["camera"],
        size: 3 << 10,
    },
    Package {
        name: "maps",
        version: "1.0.0",
        depends: &["weather"],
        grants: &["location", "network"],
        size: 7 << 10,
    },
];

pub fn find(name: &str) -> Option<&'static Package> {
    REPOSITORY.iter().find(|p| p.name == name)
}

/// The object's first line, which `pkg` checks against the index entry.
pub fn header(package: &Package) -> String {
    format!("kpkg 1 {} {}\n", package.name, package.version)
}

/// The object as the repository serves it.
pub fn object(package: &Package) -> Vec<u8> {
    let mut bytes = header(package).into_bytes();
    let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ package.size as u64;
    for byte in package.name.bytes() {
        x = (x ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    while bytes.len() < package.size {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        bytes.push(x as u8);
    }
    bytes
}

/// Lower-case hex SHA-256.
pub fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The index entry's hash for `package`.
pub fn index_sha(package: &Package) -> String {
    sha(&object(package))
}

/// `names` and everything they depend on, in repository order.
pub fn closure(names: &[&str]) -> Vec<&'static Package> {
    let mut want: Vec<&str> = names.to_vec();
    let mut i = 0;
    while i < want.len() {
        if let Some(p) = find(want[i]) {
            for dep in p.depends {
                if !want.contains(dep) {
                    want.push(dep);
                }
            }
        }
        i += 1;
    }
    REPOSITORY
        .iter()
        .filter(|p| want.contains(&p.name))
        .collect()
}
