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

/// The kernel's own requests to `fs`, on a plain route (badge 0), for the
/// `part` case: `check`, and S04-2's transfer over `kai-fs 1`'s message size.
/// The file lives in kaifs, `/system/s04-2`; every other caller speaks
/// `kai-fs 1` on a `Dir` or `File` handle.
pub mod stub {
    /// `check`: the `part` case's checks inside `fs`. The reply is
    /// `ok sectors=N first=HEX` (the first 16 bytes of the partition) or
    /// `fail <why>`.
    pub const CHECK: &[u8] = b"check";
    /// `W`, then the byte offset (u64 LE), then the bytes to write there, one
    /// kaifs write in an action that the next `F` commits. Replies `ok`.
    pub const WRITE: u8 = b'W';
    /// `R`, then the byte offset (u64 LE) and length (u32 LE): replies the bytes,
    /// read through kaifs.
    pub const READ: u8 = b'R';
    /// `F`: commits the writes since the last one, two flushes. Replies `ok`.
    pub const FLUSH: u8 = b'F';
    /// The most file bytes one message carries, as `kai-fs 1`'s messages.
    pub const MAX_DATA: usize = 7 * 1024;
}

const _: () = {
    assert!(core::mem::size_of::<PartInfo>() == 24);
    assert!(MAX_IO.is_multiple_of(SECTOR));
};
