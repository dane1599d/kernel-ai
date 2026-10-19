//! The model disk, as the kernel, ring 3 and xtask all see it: its serial, what
//! xtask tells the kernel about the file behind it, and the hash each side
//! computes over its bytes, so a test can tell the guest read exactly the file
//! the host attached.

use alloc::format;
use alloc::string::String;

/// The virtio-blk serial (device ID) of the disk holding the model file.
pub const SERIAL: &str = "kai-model";

/// What xtask tells the kernel about the file behind the model disk, on the kernel
/// command line: `model_len=BYTES`, the file's exact length (the disk rounds it up
/// to whole sectors), and for test boots `model_fnv=HEX`, its [`Fnv1a`] hash.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attached {
    pub len: Option<u64>,
    pub fnv: Option<u64>,
}

impl Attached {
    /// Reads both from a kernel command line; a malformed value counts as absent.
    pub fn from_cmdline(cmdline: &str) -> Self {
        let mut attached = Self::default();
        for word in cmdline.split_whitespace() {
            if let Some(value) = word.strip_prefix("model_len=") {
                attached.len = value.parse().ok().filter(|&len| len > 0);
            } else if let Some(value) = word.strip_prefix("model_fnv=") {
                attached.fnv = u64::from_str_radix(value, 16).ok();
            }
        }
        attached
    }

    /// The command-line words for the fields that are set.
    pub fn to_cmdline(&self) -> String {
        let len = self.len.map(|len| format!("model_len={len}"));
        let fnv = self.fnv.map(|fnv| format!("model_fnv={fnv:016x}"));
        match (len, fnv) {
            (Some(len), Some(fnv)) => format!("{len} {fnv}"),
            (Some(one), None) | (None, Some(one)) => one,
            (None, None) => String::new(),
        }
    }
}

/// FNV-1a, 64 bits, over bytes fed in any number of pieces. A check that the
/// bytes arrived unchanged, not a defence against a crafted file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fnv1a(u64);

impl Fnv1a {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    pub const fn new() -> Self {
        Self(Self::OFFSET_BASIS)
    }

    pub fn update(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(Self::PRIME);
        }
    }

    pub const fn finish(self) -> u64 {
        self.0
    }
}

impl Default for Fnv1a {
    fn default() -> Self {
        Self::new()
    }
}

/// The hash of `bytes` in one go.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = Fnv1a::new();
    hash.update(bytes);
    hash.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_matches_the_published_vectors() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn attached_facts_round_trip_through_the_command_line() {
        let both = Attached {
            len: Some(3_146_505),
            fnv: Some(0x00ab_cdef_0123_4567),
        };
        let words = both.to_cmdline();
        assert_eq!(words, "model_len=3146505 model_fnv=00abcdef01234567");
        assert_eq!(
            Attached::from_cmdline(&format!("test=blk {words} llm=10.0.2.2:80")),
            both
        );
        let len_only = Attached {
            len: Some(5),
            fnv: None,
        };
        assert_eq!(Attached::from_cmdline(&len_only.to_cmdline()), len_only);
        assert_eq!(Attached::default().to_cmdline(), "");
    }

    #[test]
    fn malformed_or_missing_facts_are_absent() {
        assert_eq!(Attached::from_cmdline("test"), Attached::default());
        let bad = Attached::from_cmdline("model_len=0 model_fnv=xyz model_lenx=4");
        assert_eq!(bad, Attached::default());
        assert_eq!(Attached::from_cmdline("model_len=-1").len, None);
    }

    #[test]
    fn pieces_hash_like_the_whole() {
        let bytes: [u8; 1000] = core::array::from_fn(|i| (i * 7 + 3) as u8);
        let mut pieces = Fnv1a::new();
        for piece in bytes.chunks(333) {
            pieces.update(piece);
        }
        assert_eq!(pieces.finish(), fnv1a(&bytes));
    }
}
