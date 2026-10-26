//! Format 1's on-disk structures (ADR 0004 §2), read and written with zerocopy:
//! the 64-byte `BlockPtr` and the 4 KiB superblock with its two rings of eight
//! slots. Fields are little-endian as stored; every target kaifs runs on is
//! little-endian, so the spike keeps native integers.

use crate::csum::CsumKind;
use crate::{BLOCK, Block, SLOTS};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// A child's address, length, generation and checksum: the checksum sits in the
/// parent, so the tree is a Merkle tree.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout,
)]
#[repr(C)]
pub struct BlockPtr {
    pub addr: u64,
    /// The generation that wrote the block.
    pub birth: u64,
    /// Record length in blocks, 1-512.
    pub blocks: u32,
    pub kind: u8,
    /// The node's height in its tree; 0 for leaves and records.
    pub level: u8,
    pub flags: u16,
    pub csum: [u8; 32],
    pub reserved: [u8; 8],
}

const _: () = assert!(size_of::<BlockPtr>() == 64);

/// `BlockPtr::kind`.
pub const KIND_NODE: u8 = 1;
pub const KIND_RECORD: u8 = 2;
pub const KIND_ALLOC: u8 = 3;

/// `BlockPtr::flags`.
pub const FLAG_SHARED: u16 = 1 << 0;
pub const FLAG_SEALED: u16 = 1 << 1;

impl BlockPtr {
    pub fn is_null(&self) -> bool {
        self.blocks == 0
    }
}

pub const MAGIC: [u8; 8] = *b"KAIFS\0\r\n";
pub const FORMAT_MAJOR: u32 = 1;
/// The oldest format this build mounts (Fxfs's `EARLIEST_SUPPORTED`).
pub const EARLIEST_SUPPORTED: u32 = 1;
/// Incompat bits this build knows; any other refuses the mount.
pub const INCOMPAT_KNOWN: u64 = 0;

/// The superblock's fields; the block holds them, then zeros, then the checksum
/// of everything before it in its last 32 bytes.
#[derive(Clone, Copy, Debug, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct Superblock {
    pub magic: [u8; 8],
    pub format_major: u32,
    pub format_minor: u32,
    pub compat: u64,
    pub ro_compat: u64,
    pub incompat: u64,
    pub uuid: [u8; 16],
    pub generation: u64,
    pub block_count: u64,
    pub block_size: u32,
    pub csum_kind: u8,
    pub pad: [u8; 3],
    pub volumes: BlockPtr,
    pub alloc: BlockPtr,
    pub gc: BlockPtr,
    pub keys: BlockPtr,
    pub commit_time: u64,
    pub label: [u8; 64],
}

const CSUM_AT: usize = BLOCK - 32;

impl Superblock {
    /// The block this superblock is written as.
    pub fn encode(&self, kind: CsumKind) -> Block {
        let mut block = [0; BLOCK];
        let bytes = self.as_bytes();
        block[..bytes.len()].copy_from_slice(bytes);
        let csum = kind.sum(&block[..CSUM_AT]);
        block[CSUM_AT..].copy_from_slice(&csum);
        block
    }

    /// The superblock in `block`, if its magic and its own checksum hold.
    pub fn decode(block: &Block) -> Option<Self> {
        let (sb, _) = Self::read_from_prefix(block).ok()?;
        if sb.magic != MAGIC {
            return None;
        }
        let kind = CsumKind::from_u8(sb.csum_kind)?;
        if kind.sum(&block[..CSUM_AT])[..] != block[CSUM_AT..] {
            return None;
        }
        Some(sb)
    }
}

/// The two blocks slot `gen mod 8` lives in: one in the ring at blocks 1-8, one in
/// the mirror ring in the last eight blocks.
pub fn slot_addrs(generation: u64, block_count: u64) -> [u64; 2] {
    let slot = generation % SLOTS;
    [1 + slot, block_count - SLOTS + slot]
}

/// Every slot block of both rings.
pub fn all_slots(block_count: u64) -> impl Iterator<Item = u64> {
    (0..SLOTS).flat_map(move |slot| [1 + slot, block_count - SLOTS + slot])
}

/// The first and one-past-last block the allocator hands out: block 0 stays
/// outside the filesystem, and the rings bound the rest.
pub fn data_area(block_count: u64) -> (u64, u64) {
    (1 + SLOTS, block_count - SLOTS)
}
