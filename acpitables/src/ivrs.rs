//! IVRS, the I/O Virtualization Reporting Structure (AMD IOMMU specification,
//! doc 48882 rev 3.10, section 5): where each AMD-Vi IOMMU is, and which
//! devices it covers with which device table settings. ADR 0007 §5's first
//! step for `amdvi`; nothing in the kernel reads it yet.
//!
//! The table is untrusted: [`parse`] takes a whole table, checks its
//! signature, length and checksum, and then reads only inside it. A block or
//! a device entry of a type it does not know is skipped by its length, and
//! one that is cut short or does not add up ends that block, never the
//! table.
//!
//! What it reads:
//! - IVHD blocks of types 10h, 11h and 40h (one IOMMU each). Firmware may
//!   describe one IOMMU with several of them, and [`Ivrs::iommus`] keeps the
//!   highest type for each.
//! - Their device entries: All (1h), Select (2h), a range (3h and 4h), the
//!   alias forms (42h and 43h), the extended forms (46h and 47h), Special
//!   (48h, an I/O APIC or HPET), and ACPI devices (F0h, type 40h only).
//! - IVMD blocks (20h to 22h) are skipped: no memory definitions are needed
//!   yet.

use crate::{SDT_HEADER_LEN, checksum_ok, u32_at, u64_at};
use alloc::vec::Vec;

/// Why an IVRS table was not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IvrsError {
    /// Not `IVRS`.
    Signature,
    /// The header's length is not the slice's, or is shorter than the header.
    Length,
    BadChecksum,
}

/// The table's IVinfo and its IOMMUs, as firmware lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ivrs {
    /// IVinfo: bit 0 EFRSup, bit 1 DMA remapping support (pre-boot DMA
    /// protection), bits 15:8 the physical address size, bits 22:21 the
    /// guest virtual address size, bits 7:5 the virtual address size.
    pub info: u32,
    /// Every IVHD block read, in table order.
    pub blocks: Vec<Ivhd>,
}

/// An IVHD block's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// 10h: the IOMMU feature reporting field, no EFR image.
    Fixed10,
    /// 11h: the EFR register image.
    Efr11,
    /// 40h: as 11h, with ACPI device entries (F0h) allowed.
    Acpi40,
}

/// One IOMMU, as one IVHD block describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ivhd {
    pub kind: Kind,
    pub flags: u8,
    /// The IOMMU's own PCI function, `bus << 8 | device << 3 | function`.
    pub device_id: u16,
    /// Its capability block's offset in that function's configuration space.
    pub capability: u16,
    /// Its MMIO registers.
    pub base: u64,
    pub segment: u16,
    /// IOMMU info: bits 4:0 the MSI number, bits 12:8 the unit ID.
    pub iommu_info: u16,
    /// 10h: IOMMU feature reporting; 11h and 40h: IOMMU attributes.
    pub attributes: u32,
    /// 11h and 40h: the EFR register image; 0 for 10h.
    pub efr: u64,
    /// Its device entries, resolved into ranges, in table order.
    pub devices: Vec<Devices>,
    /// I/O APICs and HPETs (48h).
    pub specials: Vec<Special>,
    /// ACPI devices (F0h).
    pub acpi: Vec<AcpiDevice>,
}

/// Device IDs `first..=last` and the device table setting they get: the DTE
/// setting byte (bit 0 INITPass, bit 1 EIntPass, bit 2 NMIPass, bits 5:4
/// SysMgt, bit 6 Lint0Pass, bit 7 Lint1Pass), the requester ID their DMA
/// arrives as (`alias`, from 42h and 43h), and the extended setting (46h
/// and 47h, bit 0 ATS disabled).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Devices {
    pub first: u16,
    pub last: u16,
    pub setting: u8,
    pub alias: Option<u16>,
    pub extended: u32,
}

/// An I/O APIC (`variety` 1) or HPET (2) behind the IOMMU (48h): its ID as
/// the MADT or HPET table gives it (`handle`) and the requester ID it uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Special {
    pub setting: u8,
    pub handle: u8,
    pub source: u16,
    pub variety: u8,
}

/// An ACPI device behind the IOMMU (F0h, IVHD 40h only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcpiDevice {
    pub device_id: u16,
    pub setting: u8,
    pub hid: [u8; 8],
    pub cid: [u8; 8],
    /// The UID's format (0 none, 1 an integer, 2 a string) and its bytes, at
    /// most [`MAX_UID`].
    pub uid_format: u8,
    pub uid: [u8; MAX_UID],
    pub uid_len: u8,
}

/// The longest ACPI device UID kept; a longer one is cut there (its entry is
/// still skipped by its own length).
pub const MAX_UID: usize = 32;

/// IVinfo, then 8 reserved bytes, then the blocks.
const FIRST_BLOCK: usize = SDT_HEADER_LEN + 12;
/// An IVHD block's fixed fields before its device entries.
const IVHD10_LEN: usize = 24;
const IVHD11_LEN: usize = 40;

impl Ivrs {
    /// The IOMMUs to use: for each (segment, IOMMU device ID), the block of
    /// the highest type, as the specification says software that knows it
    /// should take; among blocks of one type, the first.
    pub fn iommus(&self) -> Vec<&Ivhd> {
        let mut out: Vec<&Ivhd> = Vec::new();
        for block in &self.blocks {
            match out
                .iter_mut()
                .find(|b| b.segment == block.segment && b.device_id == block.device_id)
            {
                Some(kept) if block.kind > kept.kind => *kept = block,
                Some(_) => {}
                None => out.push(block),
            }
        }
        out
    }
}

impl Ivhd {
    /// The setting for device ID `id`: the last entry that covers it, since a
    /// Select may follow an All, or none.
    pub fn device(&self, id: u16) -> Option<Devices> {
        self.devices
            .iter()
            .rev()
            .find(|d| d.first <= id && id <= d.last)
            .copied()
    }
}

/// Reads a whole IVRS table.
pub fn parse(table: &[u8]) -> Result<Ivrs, IvrsError> {
    if table.len() < FIRST_BLOCK || u32_at(table, 4) as usize != table.len() {
        return Err(IvrsError::Length);
    }
    if &table[..4] != b"IVRS" {
        return Err(IvrsError::Signature);
    }
    if !checksum_ok(table) {
        return Err(IvrsError::BadChecksum);
    }
    let mut ivrs = Ivrs {
        info: u32_at(table, SDT_HEADER_LEN),
        blocks: Vec::new(),
    };
    let mut at = FIRST_BLOCK;
    while at + 4 <= table.len() {
        let len = usize::from(u16_at(table, at + 2));
        if len < 4 || at + len > table.len() {
            break;
        }
        if let Some(block) = ivhd(&table[at..at + len]) {
            ivrs.blocks.push(block);
        }
        // IVMD (20h to 22h) and unknown types are skipped.
        at += len;
    }
    Ok(ivrs)
}

/// An IVHD block, or `None` for another block type or one too short for its
/// fixed fields.
fn ivhd(block: &[u8]) -> Option<Ivhd> {
    let (kind, fixed) = match block[0] {
        0x10 => (Kind::Fixed10, IVHD10_LEN),
        0x11 => (Kind::Efr11, IVHD11_LEN),
        0x40 => (Kind::Acpi40, IVHD11_LEN),
        _ => return None,
    };
    if block.len() < fixed {
        return None;
    }
    let mut ivhd = Ivhd {
        kind,
        flags: block[1],
        device_id: u16_at(block, 4),
        capability: u16_at(block, 6),
        base: u64_at(block, 8),
        segment: u16_at(block, 16),
        iommu_info: u16_at(block, 18),
        attributes: u32_at(block, 20),
        efr: if kind == Kind::Fixed10 {
            0
        } else {
            u64_at(block, 24)
        },
        devices: Vec::new(),
        specials: Vec::new(),
        acpi: Vec::new(),
    };
    entries(&block[fixed..], &mut ivhd);
    Some(ivhd)
}

/// A range's start (3h, 43h or 47h), waiting for its end (4h).
struct Open {
    first: u16,
    setting: u8,
    alias: Option<u16>,
    extended: u32,
}

/// The device entries of one IVHD block. Entry types 00h to 3Fh are 4 bytes,
/// 40h to 7Fh are 8, and 80h to FFh carry their length (only F0h is
/// defined). A range's end with no start, an end below its start, or a start
/// that another start or the block's end follows is dropped.
fn entries(bytes: &[u8], ivhd: &mut Ivhd) {
    let mut open: Option<Open> = None;
    let mut at = 0;
    while at < bytes.len() {
        let kind = bytes[at];
        let len = match kind {
            0x00..=0x3f => 4,
            0x40..=0x7f => 8,
            0xf0 => match bytes.get(at + 21) {
                Some(&uid_len) => 22 + usize::from(uid_len),
                None => break,
            },
            // No other variable-length type is defined, so its length is
            // unknown: nothing after it can be read.
            _ => break,
        };
        let Some(entry) = bytes.get(at..at + len) else {
            break;
        };
        at += len;
        let id = u16_at(entry, 1);
        let setting = entry[3];
        let one = |alias, extended| Devices {
            first: id,
            last: id,
            setting,
            alias,
            extended,
        };
        match kind {
            0x01 => ivhd.devices.push(Devices {
                first: 0,
                last: u16::MAX,
                setting,
                alias: None,
                extended: 0,
            }),
            0x02 => ivhd.devices.push(one(None, 0)),
            0x42 => ivhd.devices.push(one(Some(u16_at(entry, 5)), 0)),
            0x46 => ivhd.devices.push(one(None, u32_at(entry, 4))),
            0x03 | 0x43 | 0x47 => {
                open = Some(Open {
                    first: id,
                    setting,
                    alias: (kind == 0x43).then(|| u16_at(entry, 5)),
                    extended: if kind == 0x47 { u32_at(entry, 4) } else { 0 },
                });
            }
            0x04 => {
                if let Some(start) = open.take()
                    && start.first <= id
                {
                    ivhd.devices.push(Devices {
                        first: start.first,
                        last: id,
                        setting: start.setting,
                        alias: start.alias,
                        extended: start.extended,
                    });
                }
            }
            0x48 => ivhd.specials.push(Special {
                setting,
                handle: entry[4],
                source: u16_at(entry, 5),
                variety: entry[7],
            }),
            0xf0 if ivhd.kind == Kind::Acpi40 => {
                let uid_len = usize::from(entry[21]);
                let kept = uid_len.min(MAX_UID);
                let mut uid = [0; MAX_UID];
                uid[..kept].copy_from_slice(&entry[22..22 + kept]);
                ivhd.acpi.push(AcpiDevice {
                    device_id: id,
                    setting,
                    hid: entry[4..12].try_into().unwrap_or([0; 8]),
                    cid: entry[12..20].try_into().unwrap_or([0; 8]),
                    uid_format: entry[20],
                    uid,
                    uid_len: kept as u8,
                });
            }
            // Padding (00h), reserved types, and F0h outside a 40h block.
            _ => {}
        }
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

#[cfg(test)]
mod tests;
