//! Partitions (ADR 0004 §6-7): what a `Partition` handle names and what
//! [`crate::call::PART_INFO`] reports. Only the `fs` service holds one, and only
//! for `kai-data`: no handle names the ESP or the system slots.

/// The object a `Partition` handle names: the `kai-data` partition, found by its
/// type GUID.
pub const DATA: u64 = 1;

/// Bytes in a sector: every offset and length the PART_* calls take is in them.
pub const SECTOR: u64 = 512;

/// The most one PART_READ or PART_WRITE moves: one kaifs record.
pub const MAX_IO: u64 = 128 * 1024;

/// Filled in by [`crate::call::PART_INFO`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct PartInfo {
    /// The partition's length in sectors; sector 0 is its first.
    pub sectors: u64,
    /// The most sectors one call moves: [`MAX_IO`] / [`SECTOR`].
    pub max_sectors: u64,
    /// [`WRITABLE`] and [`VOLATILE_CACHE`].
    pub flags: u64,
}

/// The disk takes writes: it can make them durable (ADR 0004 §6).
pub const WRITABLE: u64 = 1 << 0;
/// A write is durable only after PART_FLUSH.
pub const VOLATILE_CACHE: u64 = 1 << 1;

/// The stub `fs` service's requests (P2 task 3), until task 8's `kai-fs 1`. One
/// file lives at [`stub::FILE_BASE`] in `kai-data`, in records of
/// [`stub::RECORD`] bytes, each checksummed with seahash-64 as kaifs would.
pub mod stub {
    /// `check`: the `part` case's checks inside `fs`. The reply is
    /// `ok sectors=N first=HEX` (the first 16 bytes of the partition) or
    /// `fail <why>`.
    pub const CHECK: &[u8] = b"check";
    /// `W`, then the byte offset (u64 LE), then the bytes to write there. A
    /// record goes to the disk once its last byte arrives. Replies `ok`.
    pub const WRITE: u8 = b'W';
    /// `R`, then the byte offset (u64 LE) and length (u32 LE): replies the bytes,
    /// each record read from the disk and its checksum verified on first use.
    pub const READ: u8 = b'R';
    /// `F`: makes every whole record written so far durable and forgets the
    /// cached record, so the next read comes from the disk. Replies `ok`.
    pub const FLUSH: u8 = b'F';
    /// Where the file starts in the partition: 1 MiB, past the sectors `check`
    /// uses.
    pub const FILE_BASE: u64 = 1 << 20;
    /// The longest file.
    pub const FILE_MAX: u64 = 32 << 20;
    /// A record: kaifs's 128 KiB, one PART_WRITE.
    pub const RECORD: u64 = super::MAX_IO;
    /// The most file bytes one message carries, as S04-2's `kai-fs 1` messages.
    pub const MAX_DATA: usize = 7 * 1024;
}

const _: () = {
    assert!(core::mem::size_of::<PartInfo>() == 24);
    assert!(MAX_IO.is_multiple_of(SECTOR));
};
