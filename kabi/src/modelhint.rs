//! `kai-modelhint 1` (ADR 0004 §8, P2 task 10): the 512-byte record in sector 0
//! of `kai-data`, which kaifs leaves out, telling the kernel at boot how large
//! the active model is before any file system runs, so it can reserve the model
//! window. `fs` rewrites it after a model change commits; `cargo xtask fs import
//! --model` writes it on the host. A record that is missing or does not verify
//! means no model this boot, and `llm` says so.
//!
//! Layout, little-endian: the magic (8 bytes), the version (u32), 4 zero bytes,
//! the model's length (u64), its FNV-1a64 ([`crate::model::Fnv1a`]), its SHA-256
//! (32 bytes), zeros, and a CRC-32 (IEEE) of everything before it in the last 4
//! bytes.

/// Bytes in the record: one sector.
pub const SIZE: usize = 512;
pub const MAGIC: &[u8; 8] = b"KAIMHINT";
pub const VERSION: u32 = 1;
const CRC_AT: usize = SIZE - 4;

/// What the record says about the active model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hint {
    /// The GGUF file's exact length in bytes; never 0.
    pub len: u64,
    pub fnv: u64,
    pub sha256: [u8; 32],
}

impl Hint {
    pub fn encode(&self) -> [u8; SIZE] {
        let mut out = [0u8; SIZE];
        out[..8].copy_from_slice(MAGIC);
        out[8..12].copy_from_slice(&VERSION.to_le_bytes());
        out[16..24].copy_from_slice(&self.len.to_le_bytes());
        out[24..32].copy_from_slice(&self.fnv.to_le_bytes());
        out[32..64].copy_from_slice(&self.sha256);
        let crc = crc32(&out[..CRC_AT]);
        out[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
        out
    }

    /// The record in `sector`, or `None` for anything that is not a whole,
    /// verifying `kai-modelhint 1` naming a model of some length.
    pub fn decode(sector: &[u8]) -> Option<Hint> {
        let sector: &[u8; SIZE] = sector.get(..SIZE)?.try_into().ok()?;
        let word = |at: usize| u64::from_le_bytes(sector[at..at + 8].try_into().unwrap());
        let crc = u32::from_le_bytes(sector[CRC_AT..].try_into().unwrap());
        let version = u32::from_le_bytes(sector[8..12].try_into().unwrap());
        let ok = &sector[..8] == MAGIC
            && version == VERSION
            && crc == crc32(&sector[..CRC_AT])
            && word(16) != 0;
        ok.then(|| Hint {
            len: word(16),
            fnv: word(24),
            sha256: sector[32..64].try_into().unwrap(),
        })
    }
}

/// CRC-32 (IEEE 802.3, reflected), bit by bit: one sector, once a boot.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn a_hint_round_trips_and_any_damage_is_refused() {
        let hint = Hint {
            len: 2_497_281_120,
            fnv: 0x1502_8bc2_203c_1e19,
            sha256: [0xab; 32],
        };
        let bytes = hint.encode();
        assert_eq!(Hint::decode(&bytes), Some(hint));
        // Every single-bit flip is caught.
        for bit in 0..SIZE * 8 {
            let mut bad = bytes;
            bad[bit / 8] ^= 1 << (bit % 8);
            assert_eq!(Hint::decode(&bad), None, "bit {bit}");
        }
        assert_eq!(Hint::decode(&[0; SIZE]), None, "a blank sector");
        assert_eq!(Hint::decode(&bytes[..SIZE - 1]), None, "a short one");
        let empty = Hint { len: 0, ..hint }.encode();
        assert_eq!(Hint::decode(&empty), None, "no model");
    }
}
