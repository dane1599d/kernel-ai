//! The MMIO registers the kernel needs to start the command buffer and the
//! event log (doc 48882 rev 3.10, section 3.3), at offsets from the IOMMU's
//! base address (IVHD's, from `acpitables::ivrs`). Only the values are made
//! here; the kernel does the reads and writes.

use crate::{ADDRESS_MASK, page_address};

pub const DEVICE_TABLE_BASE: usize = 0x0000;
pub const COMMAND_BUFFER_BASE: usize = 0x0008;
pub const EVENT_LOG_BASE: usize = 0x0010;
pub const CONTROL: usize = 0x0018;
pub const EXTENDED_FEATURES: usize = 0x0030;
pub const COMMAND_HEAD: usize = 0x2000;
pub const COMMAND_TAIL: usize = 0x2008;
pub const EVENT_HEAD: usize = 0x2010;
pub const EVENT_TAIL: usize = 0x2018;
pub const STATUS: usize = 0x2020;

/// Control register bits.
pub mod control {
    pub const IOMMU_EN: u64 = 1 << 0;
    pub const HT_TUN_EN: u64 = 1 << 1;
    pub const EVENT_LOG_EN: u64 = 1 << 2;
    pub const EVENT_INT_EN: u64 = 1 << 3;
    pub const COM_WAIT_INT_EN: u64 = 1 << 4;
    pub const CMD_BUF_EN: u64 = 1 << 12;
}

/// Status register bits. The first three are cleared by writing 1.
pub mod status {
    pub const EVENT_OVERFLOW: u64 = 1 << 0;
    pub const EVENT_LOG_INT: u64 = 1 << 1;
    pub const COM_WAIT_INT: u64 = 1 << 2;
    pub const EVENT_LOG_RUN: u64 = 1 << 3;
    pub const CMD_BUF_RUN: u64 = 1 << 4;
}

/// The Device Table Base Address register: the table at `phys`, of `bytes`
/// (whole 4 KiB pages, at most 2 MiB), in bits 51:12, and its size in
/// pages less one in bits 8:0.
pub fn device_table_base(phys: u64, bytes: usize) -> Option<u64> {
    let pages = bytes / 4096;
    (page_address(phys) && bytes.is_multiple_of(4096) && (1..=512).contains(&pages))
        .then(|| phys | (pages as u64 - 1))
}

/// The Command Buffer or Event Log Base Address register: the ring at
/// `phys`, of `entries` entries (a power of two from 256 to 32768), in bits
/// 51:12, and log2 of `entries` in bits 59:56.
pub fn ring_base(phys: u64, entries: u32) -> Option<u64> {
    (page_address(phys) && entries.is_power_of_two() && (256..=32768).contains(&entries))
        .then(|| phys & ADDRESS_MASK | u64::from(entries.trailing_zeros()) << 56)
}
