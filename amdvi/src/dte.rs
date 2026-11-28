//! The device table entry (doc 48882 rev 3.10, section 2.2.2): 256 bits per
//! device ID, as four little-endian `u64`s, in a table the Device Table Base
//! Address register points at. This module writes and reads the DMA fields:
//!
//! | Bits    | Field                                          |
//! |---------|------------------------------------------------|
//! | 0       | V: the entry is valid                          |
//! | 1       | TV: its translation fields are valid           |
//! | 11:9    | Mode: 0 = no translation, 1-6 = levels, 7 reserved |
//! | 51:12   | Host Page Table Root Pointer, bits 51:12       |
//! | 61, 62  | IR, IW: reads and writes are allowed           |
//! | 79:64   | DomainID                                       |
//! | 96      | I: the device's IOTLB is used (ATS)            |
//! | 97, 98  | SE, SA: suppress the device's events, or all   |
//!
//! The interrupt fields (bits 128-191) are left zero, IV clear: interrupt
//! remapping is a later change (ADR 0007 section 5), and until then a
//! device's interrupts pass as it sends them.

use crate::{ADDRESS_MASK, bit, field, page_address};

/// Bytes in one entry.
pub const ENTRY_BYTES: usize = 32;
/// Device IDs one table can cover: the 16-bit requester ID.
pub const MAX_DEVICES: usize = 1 << 16;

const V: u32 = 0;
const TV: u32 = 1;
const MODE: (u32, u32) = (9, 11);
const IR: u32 = 61;
const IW: u32 = 62;
const IOTLB: u32 = 32;
const SE: u32 = 33;
const SA: u32 = 34;

/// How the device's DMA addresses are translated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paging {
    /// Mode 0: addresses are used as they are, if IR and IW allow it.
    Off,
    /// Mode 1-6: a host page table of `levels` levels, rooted at `root`.
    Tables { levels: u8, root: u64 },
}

/// The DMA half of a device table entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceEntry {
    pub valid: bool,
    pub translation_valid: bool,
    pub paging: Paging,
    pub read: bool,
    pub write: bool,
    pub domain: u16,
    pub iotlb: bool,
    pub suppress_events: bool,
    pub suppress_all: bool,
}

/// Why an entry cannot be written, or what it holds cannot be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DteError {
    /// Levels outside 1-6, or Mode 7 (reserved) read back.
    Levels(u8),
    /// A root not 4 KiB aligned, or above bit 51.
    Root(u64),
}

impl DeviceEntry {
    /// A kernel driver's device until v1.0 (ADR 0007 section 5): valid,
    /// untranslated, reads and writes allowed.
    pub const fn pass_through(domain: u16) -> Self {
        Self {
            valid: true,
            translation_valid: true,
            paging: Paging::Off,
            read: true,
            write: true,
            domain,
            iotlb: false,
            suppress_events: false,
            suppress_all: false,
        }
    }

    /// A device that may not reach memory at all: untranslated, with neither
    /// reads nor writes allowed, so each access faults.
    pub const fn blocked(domain: u16) -> Self {
        let mut entry = Self::pass_through(domain);
        entry.read = false;
        entry.write = false;
        entry
    }

    /// A confined device: its addresses go through the page table at `root`.
    pub fn paged(domain: u16, levels: u8, root: u64) -> Result<Self, DteError> {
        let entry = Self {
            paging: Paging::Tables { levels, root },
            ..Self::pass_through(domain)
        };
        entry.encode()?;
        Ok(entry)
    }

    /// The four words, in the order they sit in the table.
    pub fn encode(&self) -> Result<[u64; 4], DteError> {
        let (mode, root) = match self.paging {
            Paging::Off => (0, 0),
            Paging::Tables { levels, root } => {
                if !(1..=6).contains(&levels) {
                    return Err(DteError::Levels(levels));
                }
                if !page_address(root) {
                    return Err(DteError::Root(root));
                }
                (u64::from(levels), root)
            }
        };
        let flag = |on: bool, at: u32| u64::from(on) << at;
        let low = flag(self.valid, V)
            | flag(self.translation_valid, TV)
            | mode << MODE.0
            | root
            | flag(self.read, IR)
            | flag(self.write, IW);
        let second = u64::from(self.domain)
            | flag(self.iotlb, IOTLB)
            | flag(self.suppress_events, SE)
            | flag(self.suppress_all, SA);
        Ok([low, second, 0, 0])
    }

    /// The entry as the table holds it, little-endian.
    pub fn to_bytes(&self) -> Result<[u8; ENTRY_BYTES], DteError> {
        let mut bytes = [0; ENTRY_BYTES];
        for (chunk, word) in bytes.as_chunks_mut::<8>().0.iter_mut().zip(self.encode()?) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        Ok(bytes)
    }

    /// The DMA fields of `words`; every other bit is ignored.
    pub fn decode(words: [u64; 4]) -> Result<Self, DteError> {
        let [low, second, ..] = words;
        let paging = match field(low, MODE.0, MODE.1) as u8 {
            0 => Paging::Off,
            7 => return Err(DteError::Levels(7)),
            levels => Paging::Tables {
                levels,
                root: low & ADDRESS_MASK,
            },
        };
        Ok(Self {
            valid: bit(low, V),
            translation_valid: bit(low, TV),
            paging,
            read: bit(low, IR),
            write: bit(low, IW),
            domain: second as u16,
            iotlb: bit(second, IOTLB),
            suppress_events: bit(second, SE),
            suppress_all: bit(second, SA),
        })
    }
}

/// Bytes of a device table for device IDs `0..=last`, rounded up to whole
/// 4 KiB pages as the base register's size field counts them.
pub const fn table_bytes(last: u16) -> usize {
    let bytes = (last as usize + 1) * ENTRY_BYTES;
    bytes.div_ceil(4096) * 4096
}
