//! S07-2 (ADR 0007): AMD-Vi's in-memory formats for a minimal setup, with no
//! OS in them. Throwaway, on `spike/amdvi-qemu` only.
//!
//! - The MMIO registers' offsets and the control bits that turn the IOMMU,
//!   its command buffer and its event log on.
//! - A device table entry: translation on, a 4-level I/O page table, read
//!   and write allowed at the top, one domain; interrupts not remapped.
//! - I/O page table entries, and [`identity`]: guest memory mapped 1:1 in
//!   2 MiB pages except one 2 MiB hole, the canary's.
//! - The three commands the spike sends: COMPLETION_WAIT, INVALIDATE_DEVTAB_ENTRY
//!   and INVALIDATE_IOMMU_PAGES.
//! - Event log entries, IO_PAGE_FAULT among them.
//!
//! The source is AMD's IOMMU specification (pub. 48882, section 2 and 3), as
//! QEMU's `hw/i386/amd_iommu.c` (11.0) reads it, since QEMU is what the spike
//! runs on.

#![no_std]

/// Register offsets in the IOMMU's MMIO window.
pub mod reg {
    /// Device table base: bits 51:12 the address, 8:0 its size in 4 KiB pages
    /// less one.
    pub const DEVTAB_BASE: u64 = 0x0000;
    /// Command buffer base: bits 51:12 the address, 59:56 log2 of its entries.
    pub const CMDBUF_BASE: u64 = 0x0008;
    /// Event log base: as the command buffer's.
    pub const EVTLOG_BASE: u64 = 0x0010;
    pub const CONTROL: u64 = 0x0018;
    pub const CMDBUF_HEAD: u64 = 0x2000;
    pub const CMDBUF_TAIL: u64 = 0x2008;
    pub const EVTLOG_HEAD: u64 = 0x2010;
    pub const EVTLOG_TAIL: u64 = 0x2018;
    pub const STATUS: u64 = 0x2020;
    /// The window's size.
    pub const WINDOW: u64 = 0x4000;
}

/// Control register bits.
pub mod control {
    pub const IOMMU_EN: u64 = 1 << 0;
    pub const EVENT_LOG_EN: u64 = 1 << 2;
    pub const CMD_BUF_EN: u64 = 1 << 12;
}

/// Status register bits.
pub mod status {
    pub const EVENT_OVERFLOW: u64 = 1 << 0;
    pub const EVENT_LOG_INT: u64 = 1 << 1;
    pub const COM_WAIT_INT: u64 = 1 << 2;
    pub const EVENT_LOG_RUN: u64 = 1 << 3;
    pub const CMD_BUF_RUN: u64 = 1 << 4;
}

/// A device table entry's size, and the table for every device ID: 65,536
/// entries, 2 MiB.
pub const DTE_BYTES: u64 = 32;
pub const DEVTAB_BYTES: u64 = DTE_BYTES << 16;
/// Command and event entries are 16 bytes; 256 of them fill 4 KiB.
pub const ENTRY_BYTES: u64 = 16;
pub const RING_BYTES: u64 = 4096;
/// log2 of 256 entries, for the base registers' length fields.
pub const RING_LOG2: u64 = 8;

/// The device table base register's value for a 2 MiB table at `phys`.
pub const fn devtab_base(phys: u64) -> u64 {
    (phys & ADDRESS) | (DEVTAB_BYTES / 4096 - 1)
}

/// A 4 KiB ring's base register value: the command buffer's or event log's.
pub const fn ring_base(phys: u64) -> u64 {
    (phys & ADDRESS) | RING_LOG2 << 56
}

/// Bits 51:12: a table's or page's address.
pub const ADDRESS: u64 = 0x000F_FFFF_FFFF_F000;
/// Read and write permission, in a DTE's first quadword and every PTE.
pub const IR: u64 = 1 << 61;
pub const IW: u64 = 1 << 62;

/// A PCI device ID: bus, device and function, as the device table indexes.
pub const fn device_id(bus: u8, device: u8, function: u8) -> u16 {
    (bus as u16) << 8 | ((device as u16) & 0x1F) << 3 | (function as u16) & 0x7
}

/// The device table entry for a device in `domain` whose I/O page table has
/// `levels` levels (4: 48-bit IOVAs) rooted at `root`: valid, translation
/// valid, read and write allowed above the leaves. Its interrupt quadword is
/// 0 (IV clear), so MSIs are not remapped.
pub const fn dte(root: u64, levels: u8, domain: u16) -> [u64; 4] {
    let first = 1 // V
        | 1 << 1 // TV
        | ((levels as u64) & 0x7) << 9
        | (root & ADDRESS)
        | IR
        | IW;
    [first, domain as u64, 0, 0]
}

/// The DTE's paging mode (its levels), bits 11:9.
pub const fn dte_levels(first: u64) -> u8 {
    ((first >> 9) & 0x7) as u8
}

/// A page table entry pointing at the table of `next_level` at `table`,
/// with read and write allowed.
pub const fn pde(table: u64, next_level: u8) -> u64 {
    1 | ((next_level as u64) & 0x7) << 9 | (table & ADDRESS) | IR | IW
}

/// A leaf (next level 0) mapping the page or large page at `phys`.
pub const fn leaf(phys: u64) -> u64 {
    1 | (phys & ADDRESS) | IR | IW
}

const TWO_MIB: u64 = 1 << 21;
const ONE_GIB: u64 = 1 << 30;
const LEVEL4_SPAN: u64 = 1 << 39;

/// Builds a 4-level I/O page table mapping IOVA = physical over
/// `0..top` (rounded up to 2 MiB) in 2 MiB leaves, leaving out the 2 MiB at
/// `hole` (2 MiB aligned) so a DMA there faults. `alloc` gives a zeroed 4 KiB
/// table's physical address; `store(table, index, entry)` writes an entry.
/// Returns the root, or None when `alloc` ran out or `hole` is not aligned.
pub fn identity(
    top: u64,
    hole: u64,
    alloc: &mut impl FnMut() -> Option<u64>,
    store: &mut impl FnMut(u64, usize, u64),
) -> Option<u64> {
    if !hole.is_multiple_of(TWO_MIB) {
        return None;
    }
    let root = alloc()?;
    let top = top.div_ceil(TWO_MIB) * TWO_MIB;
    let mut level3 = None;
    let mut level2 = None;
    let mut at = 0;
    while at < top {
        if at.is_multiple_of(LEVEL4_SPAN) {
            let table = alloc()?;
            store(root, index(at, 4), pde(table, 3));
            level3 = Some(table);
        }
        if at.is_multiple_of(ONE_GIB) {
            let table = alloc()?;
            store(level3?, index(at, 3), pde(table, 2));
            level2 = Some(table);
        }
        if at != hole {
            store(level2?, index(at, 2), leaf(at));
        }
        at += TWO_MIB;
    }
    Some(root)
}

/// The index an IOVA takes at `level` (4 the root, 1 the 4 KiB tables).
pub const fn index(iova: u64, level: u8) -> usize {
    ((iova >> (12 + 9 * (level as u64 - 1))) & 0x1FF) as usize
}

/// Walks a table as QEMU's `amdvi_page_walk` does, from a DTE's first
/// quadword: the physical address and whether a write is allowed, or None
/// where the device's access faults. `load(table, index)` reads an entry.
pub fn translate(
    dte_first: u64,
    iova: u64,
    load: &impl Fn(u64, usize) -> u64,
) -> Option<(u64, bool)> {
    let mut level = dte_levels(dte_first);
    if dte_first & 1 == 0 || level == 0 || level > 6 {
        return None;
    }
    let mut entry = dte_first;
    let mut write = entry & IW != 0;
    loop {
        let table = entry & ADDRESS;
        entry = load(table, index(iova, level));
        if entry & 1 == 0 {
            return None;
        }
        write &= entry & IW != 0;
        let next = ((entry >> 9) & 0x7) as u8;
        if next == 0 {
            let span = 1u64 << (12 + 9 * (level as u64 - 1));
            return Some(((entry & ADDRESS & !(span - 1)) | (iova & (span - 1)), write));
        }
        if next >= level || next == 7 {
            return None;
        }
        level = next;
    }
}

/// Command opcodes (bits 63:60 of the first quadword).
pub mod opcode {
    pub const COMPLETION_WAIT: u64 = 0x1;
    pub const INVALIDATE_DEVTAB_ENTRY: u64 = 0x2;
    pub const INVALIDATE_IOMMU_PAGES: u64 = 0x3;
}

/// COMPLETION_WAIT with its store bit: once every earlier command is done,
/// the IOMMU writes `data` to the 8-byte aligned `store` address.
pub const fn completion_wait(store: u64, data: u64) -> [u64; 2] {
    [
        (store & 0x000F_FFFF_FFFF_FFF8) | 1 | opcode::COMPLETION_WAIT << 60,
        data,
    ]
}

/// INVALIDATE_DEVTAB_ENTRY for `device`.
pub const fn invalidate_dte(device: u16) -> [u64; 2] {
    [device as u64 | opcode::INVALIDATE_DEVTAB_ENTRY << 60, 0]
}

/// INVALIDATE_IOMMU_PAGES for every page of `domain`: S and PDE set, with the
/// address that means all of them.
pub const fn invalidate_domain(domain: u16) -> [u64; 2] {
    [
        (domain as u64) << 32 | opcode::INVALIDATE_IOMMU_PAGES << 60,
        0x7FFF_FFFF_FFFF_F000 | 1 << 1 | 1,
    ]
}

/// IO_PAGE_FAULT's event code.
pub const IO_PAGE_FAULT: u8 = 0x2;

/// One event log entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    /// Bits 63:60 of the first quadword.
    pub code: u8,
    /// The device's ID, bits 15:0.
    pub device: u16,
    /// Bits 59:48: the event's flags (for a page fault: RW, PR, I and more).
    pub flags: u16,
    /// The second quadword: the faulting address for a page fault.
    pub address: u64,
}

impl Event {
    pub const fn decode(entry: [u64; 2]) -> Self {
        Self {
            code: (entry[0] >> 60) as u8,
            device: entry[0] as u16,
            flags: ((entry[0] >> 48) & 0xFFF) as u16,
            address: entry[1],
        }
    }

    /// The event's name, for the log.
    pub fn name(&self) -> &'static str {
        match self.code {
            0x1 => "ILLEGAL_DEV_TABLE_ENTRY",
            0x2 => "IO_PAGE_FAULT",
            0x3 => "DEV_TAB_HARDWARE_ERROR",
            0x4 => "PAGE_TAB_HARDWARE_ERROR",
            0x5 => "ILLEGAL_COMMAND_ERROR",
            0x6 => "COMMAND_HARDWARE_ERROR",
            0x7 => "IOTLB_INV_TIMEOUT",
            0x8 => "INVALID_DEVICE_REQUEST",
            _ => "unknown",
        }
    }
}

#[cfg(test)]
mod tests;
