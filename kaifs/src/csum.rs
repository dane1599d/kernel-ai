//! `csum_kind` (ADR 0004 §2), an incompat value: how every block's checksum is made.
//!
//! Format 1 has one kind, seahash-64, decided 2026-10-26 from spikes S04-1 (#84:
//! portable SHA-256 268 MB/s on the GitHub runner, under its 300 MB/s bar) and
//! S04-2 (#95: SHA-256 was the one checksum that took `fs` under its 50 MB/s bar;
//! seahash ran at 5-12 GB/s inside `fs`). It fills the first 8 bytes of the
//! 32-byte field, little-endian; the rest is zero. A 64-bit checksum is enough to
//! catch torn and misdirected writes, which is what the field is for; another
//! kind would come with an incompat feature bit.

/// A block's checksum as it is stored: a `BlockPtr`'s `csum`, a superblock's last
/// 32 bytes.
pub type Csum = [u8; 32];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CsumKind {
    Seahash64 = 1,
}

impl CsumKind {
    /// The kind a superblock's `csum_kind` byte names, if this build knows it.
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Seahash64),
            _ => None,
        }
    }

    pub fn sum(self, bytes: &[u8]) -> Csum {
        match self {
            Self::Seahash64 => {
                let mut out = [0; 32];
                out[..8].copy_from_slice(&seahash::hash(bytes).to_le_bytes());
                out
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seahash_fills_the_first_eight_bytes() {
        let sum = CsumKind::Seahash64.sum(b"kaifs");
        assert_eq!(sum[..8], seahash::hash(b"kaifs").to_le_bytes());
        assert_eq!(sum[8..], [0; 24]);
        assert_ne!(
            CsumKind::Seahash64.sum(b"kaifs"),
            CsumKind::Seahash64.sum(b"kaifS")
        );
    }

    #[test]
    fn only_kind_one_is_known() {
        assert_eq!(CsumKind::from_u8(1), Some(CsumKind::Seahash64));
        assert_eq!(CsumKind::from_u8(0), None);
        assert_eq!(CsumKind::from_u8(2), None);
    }
}
