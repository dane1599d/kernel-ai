//! AMD SVM's data structures and encodings, with no OS in them (ADR 0006
//! item 2): what the kernel's hypervisor (ADR 0006 item 3) writes into a VMCB
//! before VMRUN and reads back after #VMEXIT. Nothing here runs an SVM
//! instruction, so all of it is tested on the host.
//!
//! - [`vmcb`]: the VMCB, a 4 KiB page: its control area and its state save
//!   area, at the offsets of AMD64 APM vol. 2 (pub. 24593) appendix B, each
//!   checked by a test.
//! - [`exit`]: `EXITCODE` values and the decoding of `EXITINFO1`/`EXITINFO2`
//!   for the exits the kernel handles itself (I/O, MSR, nested page fault).
//! - [`intercept`]: the intercept vectors as a set, and the MSR and I/O
//!   permission maps' bit positions.
//! - [`event`]: `EVENTINJ` (and `EXITINTINFO`, the same format).
//! - [`npt`]: nested page table entries and the walk's indices.
//!
//! The source is AMD64 APM vol. 2, chapter 15 and appendix B; the revision
//! was not checked (ADR 0006). Linux's `arch/x86/include/asm/svm.h` and
//! `uapi/asm/svm.h` (master, 2026-11) transcribe the same tables, and agree
//! with every offset and value here.

#![no_std]

pub mod event;
pub mod exit;
pub mod intercept;
pub mod npt;
pub mod vmcb;

pub use event::{Event, EventType};
pub use exit::ExitCode;
pub use intercept::{Intercept, Intercepts};
pub use npt::{Access, NptEntry};
pub use vmcb::{ControlArea, SaveArea, Segment, Vmcb};

/// CPUID leaf 0x8000_000A: SVM's revision, ASID count and features
/// (`cpufeat::Svm` decodes it).
pub const CPUID_SVM: u32 = 0x8000_000A;
/// EFER.SVME: SVM instructions are allowed (APM vol. 2, 15.4).
pub const EFER_SVME: u64 = 1 << 12;
/// MSR VM_HSAVE_PA: the physical address of the host's save area, a 4 KiB
/// page VMRUN writes host state to.
pub const MSR_VM_HSAVE_PA: u32 = 0xC001_0117;
/// MSR VM_CR: SVMDIS (bit 4) and LOCK (bit 3).
pub const MSR_VM_CR: u32 = 0xC001_0114;
/// VM_CR.SVMDIS: SVM is disabled (by the firmware, when LOCK is also set).
pub const VM_CR_SVMDIS: u64 = 1 << 4;
/// VM_CR.LOCK: SVMDIS can no longer be changed.
pub const VM_CR_LOCK: u64 = 1 << 3;

/// A 4 KiB page's size: a VMCB, the host save area and NPT tables are one.
pub const PAGE: u64 = 4096;

#[cfg(test)]
mod tests;
