//! Format 1's on-disk structures (ADR 0004 §2): the 64-byte [`BlockPtr`], the
//! 4 KiB [`Superblock`], its two rings of eight slots, and the mount rules.
//!
//! Everything is little-endian on disk, read and written through zerocopy
//! structs of byte-order-explicit fields, so no `unsafe` is needed and a
//! big-endian host would read the same disk. Callers see plain structs; decoding
//! checks every field a later step would trust.

use crate::csum::{Csum, CsumKind};
use crate::dev::BlockDev;
use crate::{BLOCK, Block, Error, SLOTS};
use alloc::boxed::Box;
use alloc::vec::Vec;
use zerocopy::little_endian::{U16, U32, U64};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

pub const MAGIC: [u8; 8] = *b"KAIFS\0\r\n";
pub const FORMAT_MAJOR: u32 = 1;
pub const FORMAT_MINOR: u32 = 0;
/// The oldest format this build mounts, as Fxfs records its own.
pub const EARLIEST_SUPPORTED: (u32, u32) = (1, 0);

/// Feature bits this build knows (ext4's rules, ADR 0004 §2): an unknown incompat
/// bit refuses the mount, an unknown ro_compat bit mounts read-only, an unknown
/// compat bit mounts but `check --repair` refuses. Format 1 defines none yet.
pub const INCOMPAT_KNOWN: u64 = 0;
pub const RO_COMPAT_KNOWN: u64 = 0;
pub const COMPAT_KNOWN: u64 = 0;
/// Reserved for P8's encryption (ADR 0004 §9); this build refuses it.
pub const INCOMPAT_ENCRYPTED: u64 = 1 << 0;

/// What a [`BlockPtr`] points at.
pub const KIND_NODE: u8 = 1;
pub const KIND_RECORD: u8 = 2;
pub const KIND_ALLOC: u8 = 3;
const KIND_LAST: u8 = KIND_ALLOC;

/// [`BlockPtr::flags`]: the block is reachable from more than one tree, so its
/// reuse waits for the GC; the record is a sealed blob (models, ADR 0004 §8).
pub const FLAG_SHARED: u16 = 1 << 0;
pub const FLAG_SEALED: u16 = 1 << 1;
/// Reserved: no format 1 block is compressed or encrypted.
pub const FLAG_COMPRESSED: u16 = 1 << 2;
pub const FLAG_ENCRYPTED: u16 = 1 << 3;
const FLAGS_KNOWN: u16 = FLAG_SHARED | FLAG_SEALED;

/// The longest record: 512 blocks, 2 MiB (ADR 0004 §1's sealed blobs).
pub const MAX_RECORD_BLOCKS: u32 = 512;
/// The tallest tree a pointer may claim; a 4 KiB-node tree of 2^64 items is far
/// lower.
pub const MAX_LEVEL: u8 = 16;

/// A child's address, length, generation and checksum. The checksum sits in the
/// parent, so every tree is a Merkle tree: a torn or misdirected write is caught
/// when the child is read, never returned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BlockPtr {
    pub addr: u64,
    /// The generation that wrote the block.
    pub birth: u64,
    /// The record's length in blocks, 1 to 512; 0 only in the null pointer.
    pub blocks: u32,
    pub kind: u8,
    /// The node's height in its tree: 0 for leaves and records.
    pub level: u8,
    pub flags: u16,
    pub csum: Csum,
}

#[derive(Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
struct RawPtr {
    addr: U64,
    birth: U64,
    blocks: U32,
    kind: u8,
    level: u8,
    flags: U16,
    csum: [u8; 32],
    reserved: [u8; 8],
}

/// Bytes in a stored [`BlockPtr`].
pub const PTR_BYTES: usize = 64;
const _: () = assert!(size_of::<RawPtr>() == PTR_BYTES);

impl BlockPtr {
    /// No block: an empty tree, an unused root.
    pub const NULL: BlockPtr = BlockPtr {
        addr: 0,
        birth: 0,
        blocks: 0,
        kind: 0,
        level: 0,
        flags: 0,
        csum: [0; 32],
    };

    pub fn is_null(&self) -> bool {
        self.blocks == 0
    }

    pub fn encode(&self) -> [u8; PTR_BYTES] {
        let raw = RawPtr {
            addr: self.addr.into(),
            birth: self.birth.into(),
            blocks: self.blocks.into(),
            kind: self.kind,
            level: self.level,
            flags: self.flags.into(),
            csum: self.csum,
            reserved: [0; 8],
        };
        let mut out = [0; PTR_BYTES];
        out.copy_from_slice(raw.as_bytes());
        out
    }

    /// Reads a stored pointer. The reserved bytes are not checked: the parent's
    /// checksum covers them, and a later minor version may use them.
    pub fn decode(bytes: &[u8; PTR_BYTES]) -> Result<Self, Error> {
        let raw = RawPtr::read_from_bytes(bytes).map_err(|_| Error::Corrupt("block pointer"))?;
        let ptr = BlockPtr {
            addr: raw.addr.get(),
            birth: raw.birth.get(),
            blocks: raw.blocks.get(),
            kind: raw.kind,
            level: raw.level,
            flags: raw.flags.get(),
            csum: raw.csum,
        };
        if ptr.blocks == 0 {
            return match ptr == Self::NULL {
                true => Ok(Self::NULL),
                false => Err(Error::Corrupt("a null pointer with fields set")),
            };
        }
        if ptr.flags & (FLAG_COMPRESSED | FLAG_ENCRYPTED) != 0 {
            return Err(Error::Unsupported("a compressed or encrypted block"));
        }
        if ptr.flags & !FLAGS_KNOWN != 0 {
            return Err(Error::Corrupt("unknown pointer flags"));
        }
        if ptr.blocks > MAX_RECORD_BLOCKS {
            return Err(Error::Corrupt("a record longer than 512 blocks"));
        }
        if !(KIND_NODE..=KIND_LAST).contains(&ptr.kind) {
            return Err(Error::Corrupt("unknown pointer kind"));
        }
        if ptr.level > MAX_LEVEL || (ptr.kind != KIND_NODE && ptr.level != 0) {
            return Err(Error::Corrupt("pointer level"));
        }
        if ptr.kind != KIND_RECORD && ptr.blocks != 1 {
            return Err(Error::Corrupt(
                "a node or allocator block longer than one block",
            ));
        }
        Ok(ptr)
    }

    /// A pointer read from `bytes[at..at + 64]`.
    pub fn decode_at(bytes: &[u8], at: usize) -> Result<Self, Error> {
        let slice = bytes
            .get(at..at + PTR_BYTES)
            .ok_or(Error::Corrupt("block pointer out of bounds"))?;
        let mut raw = [0; PTR_BYTES];
        raw.copy_from_slice(slice);
        Self::decode(&raw)
    }
}

/// The superblock's fields as they sit at the start of its block.
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
struct RawSuper {
    magic: [u8; 8],
    format_major: U32,
    format_minor: U32,
    compat: U64,
    ro_compat: U64,
    incompat: U64,
    uuid: [u8; 16],
    generation: U64,
    block_count: U64,
    block_size: U32,
    csum_kind: u8,
    reserved0: [u8; 3],
    volumes: RawPtr,
    alloc: RawPtr,
    gc: RawPtr,
    keys: RawPtr,
    commit_time: U64,
    label: [u8; 64],
}

const _: () = assert!(size_of::<RawSuper>() == 408);
/// The superblock's own checksum sits in its last 32 bytes, over everything before.
const SUPER_CSUM_AT: usize = BLOCK - 32;

/// One commit's root record. Slot `generation mod 8` of both rings holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Superblock {
    pub format_major: u32,
    pub format_minor: u32,
    pub compat: u64,
    pub ro_compat: u64,
    pub incompat: u64,
    pub uuid: [u8; 16],
    pub generation: u64,
    pub block_count: u64,
    pub csum_kind: CsumKind,
    /// The volume table's root.
    pub volumes: BlockPtr,
    /// The allocator's state.
    pub alloc: BlockPtr,
    /// The GC's progress, null when none is under way.
    pub gc: BlockPtr,
    /// Encryption keys: null in format 1.
    pub keys: BlockPtr,
    /// Nanoseconds since 1970 by the writer's clock; kaifs never reads it.
    pub commit_time: u64,
    /// A name for people, NUL-padded UTF-8.
    pub label: [u8; 64],
}

/// Why a slot holds no superblock this build can use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unreadable {
    /// No kaifs superblock: zeros, or another format's bytes.
    Empty,
    /// kaifs's magic, but the slot does not check: torn, or damaged.
    Damaged,
    /// A superblock this build cannot read: what it needs.
    Newer(&'static str),
}

impl Superblock {
    /// Format 1's superblock for `generation`, with null roots.
    pub fn new(uuid: [u8; 16], block_count: u64, generation: u64) -> Self {
        Superblock {
            format_major: FORMAT_MAJOR,
            format_minor: FORMAT_MINOR,
            compat: 0,
            ro_compat: 0,
            incompat: 0,
            uuid,
            generation,
            block_count,
            csum_kind: CsumKind::Seahash64,
            volumes: BlockPtr::NULL,
            alloc: BlockPtr::NULL,
            gc: BlockPtr::NULL,
            keys: BlockPtr::NULL,
            commit_time: 0,
            label: [0; 64],
        }
    }

    fn raw_ptr(ptr: &BlockPtr) -> RawPtr {
        RawPtr::read_from_bytes(&ptr.encode()).unwrap_or_else(|_| unreachable!("64 bytes"))
    }

    pub fn encode(&self) -> Block {
        let raw = RawSuper {
            magic: MAGIC,
            format_major: self.format_major.into(),
            format_minor: self.format_minor.into(),
            compat: self.compat.into(),
            ro_compat: self.ro_compat.into(),
            incompat: self.incompat.into(),
            uuid: self.uuid,
            generation: self.generation.into(),
            block_count: self.block_count.into(),
            block_size: (BLOCK as u32).into(),
            csum_kind: self.csum_kind as u8,
            reserved0: [0; 3],
            volumes: Self::raw_ptr(&self.volumes),
            alloc: Self::raw_ptr(&self.alloc),
            gc: Self::raw_ptr(&self.gc),
            keys: Self::raw_ptr(&self.keys),
            commit_time: self.commit_time.into(),
            label: self.label,
        };
        let mut block = [0; BLOCK];
        block[..size_of::<RawSuper>()].copy_from_slice(raw.as_bytes());
        let csum = self.csum_kind.sum(&block[..SUPER_CSUM_AT]);
        block[SUPER_CSUM_AT..].copy_from_slice(&csum);
        block
    }

    /// Reads one slot.
    pub fn decode(block: &Block) -> Result<Superblock, Unreadable> {
        let Ok((raw, _)) = RawSuper::read_from_prefix(block) else {
            return Err(Unreadable::Empty);
        };
        if raw.magic != MAGIC {
            return Err(Unreadable::Empty);
        }
        let Some(kind) = CsumKind::from_u8(raw.csum_kind) else {
            return Err(Unreadable::Newer("an unknown checksum kind"));
        };
        if kind.sum(&block[..SUPER_CSUM_AT])[..] != block[SUPER_CSUM_AT..] {
            return Err(Unreadable::Damaged);
        }
        if raw.block_size.get() != BLOCK as u32 {
            return Err(Unreadable::Newer("a block size other than 4 KiB"));
        }
        let ptr = |raw: &RawPtr| BlockPtr::decode_at(raw.as_bytes(), 0);
        let ptrs = (
            ptr(&raw.volumes),
            ptr(&raw.alloc),
            ptr(&raw.gc),
            ptr(&raw.keys),
        );
        let (volumes, alloc, gc, keys) = match ptrs {
            (Ok(v), Ok(a), Ok(g), Ok(k)) => (v, a, g, k),
            (v, a, g, k) => {
                return match [v, a, g, k].into_iter().find_map(Result::err) {
                    Some(Error::Unsupported(what)) => Err(Unreadable::Newer(what)),
                    _ => Err(Unreadable::Damaged),
                };
            }
        };
        if !keys.is_null() {
            return Err(Unreadable::Newer("encryption keys"));
        }
        Ok(Superblock {
            format_major: raw.format_major.get(),
            format_minor: raw.format_minor.get(),
            compat: raw.compat.get(),
            ro_compat: raw.ro_compat.get(),
            incompat: raw.incompat.get(),
            uuid: raw.uuid,
            generation: raw.generation.get(),
            block_count: raw.block_count.get(),
            csum_kind: kind,
            volumes,
            alloc,
            gc,
            keys,
            commit_time: raw.commit_time.get(),
            label: raw.label,
        })
    }

    /// The mount rules (ADR 0004 §2): another major, or a format older than
    /// [`EARLIEST_SUPPORTED`], or an unknown incompat bit refuses.
    pub fn rules(&self) -> Result<Rules, Error> {
        if self.format_major != FORMAT_MAJOR
            || (self.format_major, self.format_minor) < EARLIEST_SUPPORTED
        {
            return Err(Error::Unsupported("another format version"));
        }
        if self.incompat & !INCOMPAT_KNOWN != 0 {
            return Err(Error::Unsupported("an unknown incompat feature"));
        }
        Ok(Rules {
            writable: self.ro_compat & !RO_COMPAT_KNOWN == 0,
            repairable: self.compat & !COMPAT_KNOWN == 0,
        })
    }
}

/// What a superblock lets this build do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    /// No unknown ro_compat bit: the filesystem may be written.
    pub writable: bool,
    /// No unknown compat bit: `check --repair` may run.
    pub repairable: bool,
}

/// The fewest blocks a filesystem may have: block 0, both rings and room for the
/// first commit.
pub const MIN_BLOCKS: u64 = 64;

/// The two blocks of slot `generation mod 8`: one in the ring at blocks 1-8, one
/// in the mirror ring in the last eight blocks. Block 0 stays outside the
/// filesystem (ADR 0004 §8 puts `kai-modelhint 1` there).
pub fn slot_addrs(generation: u64, block_count: u64) -> [u64; 2] {
    let slot = generation % SLOTS;
    [1 + slot, block_count - SLOTS + slot]
}

/// Every slot block of both rings.
pub fn all_slots(block_count: u64) -> impl Iterator<Item = u64> {
    (0..SLOTS).flat_map(move |slot| [1 + slot, block_count - SLOTS + slot])
}

/// The first and one-past-last block the allocator may hand out.
pub fn data_area(block_count: u64) -> (u64, u64) {
    (1 + SLOTS, block_count - SLOTS)
}

/// Writes `sb` to its slot in both rings. No flush: the caller's commit flushes.
pub fn write_rings<D: BlockDev>(dev: &mut D, sb: &Superblock) -> Result<(), Error> {
    let block = sb.encode();
    for addr in slot_addrs(sb.generation, sb.block_count) {
        dev.write(addr, &block)?;
    }
    Ok(())
}

/// Zeroes all sixteen slots, so no superblock of an earlier filesystem on the disk
/// can outrank a new one.
pub fn clear_rings<D: BlockDev>(dev: &mut D, block_count: u64) -> Result<(), Error> {
    let zero = [0; BLOCK];
    for addr in all_slots(block_count) {
        dev.write(addr, &zero)?;
    }
    Ok(())
}

/// Reads all sixteen slots and returns the superblocks that verify, newest first,
/// one per generation and all of the newest's filesystem (its uuid). A slot this
/// build cannot read, or a newest superblock that needs a feature it lacks,
/// refuses the mount, so an older commit is never mounted in place of a newer one
/// this build does not understand.
pub fn read_rings<D: BlockDev>(dev: &mut D) -> Result<Vec<Superblock>, Error> {
    let blocks = dev.info().blocks;
    if blocks < MIN_BLOCKS {
        return Err(Error::NoValidSuperblock);
    }
    let mut buf: Box<Block> = Box::new([0; BLOCK]);
    let mut found: Vec<Superblock> = Vec::new();
    for addr in all_slots(blocks) {
        dev.read(addr, &mut buf)?;
        match Superblock::decode(&buf) {
            Err(Unreadable::Newer(what)) => return Err(Error::Unsupported(what)),
            Ok(sb) if sb.block_count == blocks => found.push(sb),
            _ => {}
        }
    }
    found.sort_by_key(|sb| core::cmp::Reverse(sb.generation));
    found.dedup_by_key(|sb| sb.generation);
    let Some(newest) = found.first().copied() else {
        return Err(Error::NoValidSuperblock);
    };
    newest.rules()?;
    found.retain(|sb| sb.uuid == newest.uuid);
    Ok(found)
}

/// Reads the one block `ptr` names, checking it lies in the data area and matches
/// its checksum.
pub fn read_checked<D: BlockDev>(
    dev: &mut D,
    kind: CsumKind,
    ptr: &BlockPtr,
    what: &'static str,
) -> Result<Box<Block>, Error> {
    let (start, end) = data_area(dev.info().blocks);
    if ptr.blocks != 1 || ptr.addr < start || ptr.addr >= end {
        return Err(Error::Corrupt(what));
    }
    let mut block: Box<Block> = Box::new([0; BLOCK]);
    dev.read(ptr.addr, &mut block)?;
    if kind.sum(&block[..]) != ptr.csum {
        return Err(Error::Corrupt(what));
    }
    Ok(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dev::MemDisk;

    fn ptr() -> BlockPtr {
        BlockPtr {
            addr: 4242,
            birth: 7,
            blocks: 1,
            kind: KIND_NODE,
            level: 2,
            flags: FLAG_SHARED,
            csum: [9; 32],
        }
    }

    #[test]
    fn a_pointer_round_trips_and_is_little_endian() {
        let bytes = ptr().encode();
        assert_eq!(bytes[..8], 4242u64.to_le_bytes());
        assert_eq!(BlockPtr::decode(&bytes), Ok(ptr()));
        assert_eq!(
            BlockPtr::decode(&BlockPtr::NULL.encode()),
            Ok(BlockPtr::NULL)
        );
    }

    #[test]
    fn a_pointer_that_breaks_a_rule_is_refused() {
        let bad = |change: fn(&mut BlockPtr)| {
            let mut p = ptr();
            change(&mut p);
            BlockPtr::decode(&p.encode())
        };
        assert!(matches!(bad(|p| p.blocks = 513), Err(Error::Corrupt(_))));
        assert!(matches!(bad(|p| p.kind = 0), Err(Error::Corrupt(_))));
        assert!(matches!(bad(|p| p.kind = 9), Err(Error::Corrupt(_))));
        assert!(matches!(bad(|p| p.flags = 1 << 9), Err(Error::Corrupt(_))));
        assert!(matches!(
            bad(|p| p.flags = FLAG_ENCRYPTED),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(bad(|p| p.level = 17), Err(Error::Corrupt(_))));
        assert!(matches!(bad(|p| p.blocks = 2), Err(Error::Corrupt(_))));
        assert!(
            bad(|p| {
                p.kind = KIND_RECORD;
                p.level = 0;
                p.blocks = 512;
            })
            .is_ok()
        );
        assert!(matches!(
            bad(|p| {
                p.blocks = 0;
                p.addr = 1;
            }),
            Err(Error::Corrupt(_))
        ));
    }

    #[test]
    fn a_superblock_round_trips_and_its_checksum_guards_it() {
        let mut sb = Superblock::new([5; 16], 4096, 77);
        sb.volumes = ptr();
        sb.label[..4].copy_from_slice(b"home");
        let mut block = sb.encode();
        assert_eq!(block[..8], MAGIC);
        assert_eq!(Superblock::decode(&block), Ok(sb));
        block[100] ^= 1;
        assert_eq!(Superblock::decode(&block), Err(Unreadable::Damaged));
        assert_eq!(Superblock::decode(&[0; BLOCK]), Err(Unreadable::Empty));
    }

    #[test]
    fn the_mount_rules_are_ext4s() {
        let sb = Superblock::new([1; 16], 4096, 1);
        let with = |f: fn(&mut Superblock)| {
            let mut s = sb;
            f(&mut s);
            s.rules()
        };
        assert_eq!(
            sb.rules(),
            Ok(Rules {
                writable: true,
                repairable: true
            })
        );
        assert!(matches!(
            with(|s| s.incompat = 1 << 40),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            with(|s| s.incompat = INCOMPAT_ENCRYPTED),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            with(|s| s.format_major = 2),
            Err(Error::Unsupported(_))
        ));
        assert!(matches!(
            with(|s| s.format_major = 0),
            Err(Error::Unsupported(_))
        ));
        assert_eq!(with(|s| s.ro_compat = 1).map(|r| r.writable), Ok(false));
        assert_eq!(
            with(|s| s.compat = 1).map(|r| (r.writable, r.repairable)),
            Ok((true, false))
        );
        assert!(with(|s| s.format_minor = 9).is_ok());
    }

    #[test]
    fn the_rings_pick_the_newest_superblock_of_the_newest_filesystem() {
        let mut disk = MemDisk::new(4096);
        let old = Superblock::new([1; 16], 4096, 40);
        let new = Superblock::new([2; 16], 4096, 3);
        write_rings(&mut disk, &old).unwrap();
        write_rings(&mut disk, &new).unwrap();
        let mut newer = new;
        newer.generation = 4;
        write_rings(&mut disk, &newer).unwrap();
        // The other filesystem's generation 40 outranks; clearing the rings is
        // what a format does first.
        assert_eq!(read_rings(&mut disk).unwrap()[0].uuid, [1; 16]);
        clear_rings(&mut disk, 4096).unwrap();
        write_rings(&mut disk, &new).unwrap();
        write_rings(&mut disk, &newer).unwrap();
        let found = read_rings(&mut disk).unwrap();
        assert_eq!(
            found.iter().map(|s| s.generation).collect::<Vec<_>>(),
            [4, 3]
        );
    }

    #[test]
    fn a_superblock_this_build_cannot_read_refuses_the_mount() {
        let mut disk = MemDisk::new(4096);
        write_rings(&mut disk, &Superblock::new([1; 16], 4096, 5)).unwrap();
        let mut future = Superblock::new([1; 16], 4096, 6);
        future.incompat = 1 << 30;
        write_rings(&mut disk, &future).unwrap();
        assert!(matches!(read_rings(&mut disk), Err(Error::Unsupported(_))));
        let mut block = Superblock::new([1; 16], 4096, 7).encode();
        block[76] = 2; // csum_kind 2, which this build does not know
        disk.write(1, &block).unwrap();
        assert!(matches!(read_rings(&mut disk), Err(Error::Unsupported(_))));
    }
}
