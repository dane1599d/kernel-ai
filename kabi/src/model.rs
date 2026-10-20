//! The model disk, as the kernel, ring 3 and xtask all see it: its serial, what
//! xtask tells the kernel about the file behind it, the kernel's whole check of
//! that file, the page tables its mapping takes, and the hash each side computes
//! over its bytes, so a test can tell the guest read exactly the file the host
//! attached.

use alloc::format;
use alloc::string::String;

/// The virtio-blk serial (device ID) of the disk holding the model file.
pub const SERIAL: &str = "kai-model";
/// The magic every GGUF file starts with.
pub const MAGIC: [u8; 4] = *b"GGUF";
/// The one GGUF version ring 3's reader takes.
pub const VERSION: u32 = 3;

/// Why the start of a disk is not a model the kernel will hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderError {
    /// Fewer than the 8 bytes of magic and version.
    Short,
    NoMagic,
    Version(u32),
}

/// The kernel's whole check of a model file: the GGUF magic and version at the
/// start of `head`. Everything after them is ring 3's to parse, as untrusted input.
pub fn check_header(head: &[u8]) -> Result<(), HeaderError> {
    let (Some(magic), Some(version)) = (head.get(..4), head.get(4..8)) else {
        return Err(HeaderError::Short);
    };
    if magic != MAGIC {
        return Err(HeaderError::NoMagic);
    }
    let version = u32::from_le_bytes([version[0], version[1], version[2], version[3]]);
    if version != VERSION {
        return Err(HeaderError::Version(version));
    }
    Ok(())
}

/// Page-table frames that mapping `len` bytes at [`crate::layout::MODEL_BASE`]
/// takes: the tables of 4 KiB pages, the directories above them, and the one table
/// under the window's PML4 entry. The window is a whole PML4 slot that holds
/// nothing else, so the count is exact for a model up to
/// [`crate::layout::MODEL_MAX`].
pub const fn table_pages(len: u64) -> u64 {
    if len == 0 {
        return 0;
    }
    let tables = len.div_ceil(4096).div_ceil(512);
    let directories = tables.div_ceil(512);
    tables + directories + 1
}

/// Positions of context the llm service's engine holds at most.
pub const CONTEXT_CAP: usize = 512;
/// Bytes the engine's f32 KV cache may take: [`CONTEXT_CAP`] positions of
/// Qwen3-0.6B (28 layers of 1,024 keys and 1,024 values), 112 MiB. A model that
/// needs more per position gets fewer positions.
pub const KV_BYTES: u64 = 112 << 20;
/// Bytes for everything else the engine sizes from the file: the tokenizer while
/// it is built, the logits, the RoPE table and the scratch vectors. The engine
/// refuses a file whose header says it would need more; Qwen3's vocabulary is
/// estimated at about 16 MiB.
pub const ENGINE_OTHER_BYTES: u64 = 24 << 20;
/// Pages the llm plan line adds for the engine when a model is loaded.
pub const ENGINE_PAGES: u64 = (KV_BYTES + ENGINE_OTHER_BYTES) / 4096;

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
    fn only_gguf_v3_passes_the_kernel_check() {
        assert_eq!(check_header(b"GGUF\x03\0\0\0rest"), Ok(()));
        assert_eq!(check_header(b"GGUF\x03\0\0\0"), Ok(()));
        assert_eq!(
            check_header(b"GGUF\x02\0\0\0"),
            Err(HeaderError::Version(2))
        );
        assert_eq!(
            check_header(b"GGUF\0\0\0\x03"),
            Err(HeaderError::Version(0x0300_0000))
        );
        assert_eq!(check_header(b"Kernel AI blk"), Err(HeaderError::NoMagic));
        assert_eq!(check_header(b"GGUF\x03"), Err(HeaderError::Short));
        assert_eq!(check_header(b""), Err(HeaderError::Short));
    }

    #[test]
    fn table_pages_count_every_level() {
        assert_eq!(table_pages(0), 0);
        assert_eq!(table_pages(1), 3);
        assert_eq!(table_pages(2 * 1024 * 1024), 3);
        assert_eq!(table_pages(2 * 1024 * 1024 + 1), 4);
        assert_eq!(table_pages(1024 * 1024 * 1024 + 1), 513 + 2 + 1);
        // Qwen3-0.6B Q8_0: 156,116 pages in 305 tables under one directory.
        assert_eq!(table_pages(639_446_688), 305 + 1 + 1);
        // The largest model: a full slot of 512 directories.
        assert_eq!(table_pages(crate::layout::MODEL_MAX), 512 * 512 + 512 + 1);
    }

    #[test]
    fn the_kv_budget_holds_the_mvp_model_at_the_context_cap() {
        let (layers, kv_dim, f32_bytes) = (28u64, 1024u64, 4u64);
        assert_eq!(
            layers * kv_dim * 2 * f32_bytes * CONTEXT_CAP as u64,
            KV_BYTES
        );
        assert_eq!(ENGINE_PAGES * 4096, KV_BYTES + ENGINE_OTHER_BYTES);
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
