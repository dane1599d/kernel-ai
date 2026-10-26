//! `csum_kind`: SHA-256 or seahash-64, each filling the 32-byte checksum field
//! (seahash in its first 8 bytes, the rest zero). S04-1 measures which to freeze.

use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CsumKind {
    Sha256 = 1,
    Seahash = 2,
}

impl CsumKind {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Sha256),
            2 => Some(Self::Seahash),
            _ => None,
        }
    }

    /// The checksum of `data` in the 32-byte field.
    pub fn sum(self, data: &[u8]) -> [u8; 32] {
        match self {
            Self::Sha256 => Sha256::digest(data).into(),
            Self::Seahash => {
                let mut out = [0; 32];
                out[..8].copy_from_slice(&seahash::hash(data).to_le_bytes());
                out
            }
        }
    }

    /// The checksum of several pieces as if they were one buffer.
    pub fn sum_parts(self, parts: &[&[u8]]) -> [u8; 32] {
        match self {
            Self::Sha256 => {
                let mut hasher = Sha256::new();
                for part in parts {
                    hasher.update(part);
                }
                hasher.finalize().into()
            }
            Self::Seahash => {
                let mut hasher = seahash::SeaHasher::new();
                for part in parts {
                    core::hash::Hasher::write(&mut hasher, part);
                }
                let mut out = [0; 32];
                out[..8].copy_from_slice(&core::hash::Hasher::finish(&hasher).to_le_bytes());
                out
            }
        }
    }
}
