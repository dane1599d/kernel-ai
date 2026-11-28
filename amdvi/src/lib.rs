//! AMD-Vi, the AMD IOMMU (ADR 0007 section 5): the formats the kernel writes
//! and reads in memory, written clean-room from AMD's IOMMU specification,
//! doc 48882 rev 3.10. Nothing here is ported from another IOMMU driver.
//!
//! - [`dte`]: the 256-bit device table entry (section 2.2.2).
//! - [`pagetable`]: the host I/O page table (section 2.2.3), built over a
//!   [`pagetable::DmaAlloc`] the kernel gives it.
//! - [`command`]: the 128-bit commands and the command buffer ring
//!   (section 2.4).
//! - [`event`]: the 128-bit event log entries (section 2.5), decoded from
//!   bytes the IOMMU wrote, so never trusted.
//! - [`regs`]: the MMIO registers the command buffer and event log need
//!   (section 3.3).
//!
//! The field layouts were written from the specification as its author knows
//! it; this change could not fetch the PDF to check each table again, so the
//! kernel's bring-up under QEMU's `amd-iommu` is their first check against an
//! implementation (ADR 0007, v0.3). No MMIO happens here, and nothing here
//! panics on what the hardware wrote.

#![no_std]

extern crate alloc;

pub mod command;
pub mod dte;
pub mod event;
pub mod pagetable;
pub mod regs;

/// Bits `lo..=hi` of `value`, shifted down.
pub(crate) const fn field(value: u64, lo: u32, hi: u32) -> u64 {
    (value >> lo) & (u64::MAX >> (63 - (hi - lo)))
}

/// Bit `bit` of `value`.
pub(crate) const fn bit(value: u64, bit: u32) -> bool {
    value >> bit & 1 == 1
}

/// Bits 51:12 of a physical address: what every pointer field holds.
pub const ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;

/// Whether `phys` fits a 4 KiB-aligned pointer field.
pub const fn page_address(phys: u64) -> bool {
    phys & !ADDRESS_MASK == 0
}
