//! Updates (ADR 0008 §10): what an `Update` handle reaches. Only the `updated`
//! service holds one. It covers the slots R, A and B and the boot record
//! (`KaiBootState`), never the ESP or `kai-data`.

/// Slot numbers, as the SLOT_* calls take them and `bootslot::Slot` numbers them.
pub const R: u64 = 0;
pub const A: u64 = 1;
pub const B: u64 = 2;
/// The update disk (ADR 0008 §12, virtio serial `kai-update`), read-only: what
/// SLOT_INFO and SLOT_READ take in place of a slot. Never written.
pub const DISK: u64 = 3;
/// "No slot": the one-shot BOOTREC_SET deletes, or the active slot off a slot disk.
pub const NONE: u64 = 0xFF;

/// The object an `Update` handle names: the slots and the boot record of the
/// boot disk.
pub const SYSTEM: u64 = 1;

/// Bytes in a sector: every offset and length the SLOT_* calls take is in them.
pub const SECTOR: u64 = 512;
/// The most one SLOT_READ or SLOT_WRITE moves.
pub const MAX_IO: u64 = 128 * 1024;
/// `KaiBootState`'s size (`bootslot::BYTES`).
pub const RECORD_BYTES: u64 = 64;

/// Filled in by [`crate::call::SLOT_INFO`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct SlotInfo {
    /// The slot partition's length in sectors; sector 0 is its first.
    pub sectors: u64,
    /// The most sectors one call moves: [`MAX_IO`] / [`SECTOR`].
    pub max_sectors: u64,
    /// The slot this boot runs in, or [`NONE`].
    pub active: u64,
    /// [`WRITABLE`] when SLOT_WRITE would take this slot now.
    pub flags: u64,
}

/// SLOT_WRITE takes this slot now: not the active one, and not R unless the
/// active slot is committed.
pub const WRITABLE: u64 = 1 << 0;
