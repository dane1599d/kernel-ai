//! Intel VMX's data structures and encodings, with no OS in them (P6, the
//! twin of the `svm` crate): what the kernel's hypervisor writes into a VMCS
//! with VMWRITE before VMLAUNCH, and reads back with VMREAD after a VM exit.
//! Nothing here runs a VMX instruction, so all of it is tested on the host.
//!
//! - [`field`]: VMCS field encodings (their width, type and index) and the
//!   fields the kernel uses, each checked by a test.
//! - [`exit`]: the exit-reason field and the exit qualifications of the exits
//!   the kernel handles itself (CR, DR and I/O access, EPT violation, entry
//!   failure).
//! - [`event`]: the interruption-information format, both the VM-entry
//!   injection field and the exit and IDT-vectoring fields.
//! - [`ept`]: EPT entries, the EPT pointer and the walk's indices.
//! - [`controls`]: the execution, exit and entry controls and the exception
//!   bitmap as a set, and their check against the capability MSRs.
//! - [`bitmap`]: the MSR bitmap and the two I/O bitmaps.
//! - [`vmcs`]: IA32_VMX_BASIC and the VMCS and VMXON regions' header.
//!
//! Every decoder of a value the CPU or a guest wrote returns an error for an
//! encoding the SDM does not define, never a panic: a VMCS field a hostile
//! guest can shape is read as data.
//!
//! The source is the Intel 64 and IA-32 Architectures Software Developer's
//! Manual, version 093 (Intel's SDM page, updated 2026-10-09): volume 3C's
//! VMX chapters, and volume 3D's appendices A (VMX capability reporting), B
//! (field encoding in VMCS) and C (VMX basic exit reasons). Linux's
//! `arch/x86/include/asm/vmx.h` and `uapi/asm/vmx.h` (master, 2026-11)
//! transcribe the same tables, and agree with every value they share with
//! this crate.

#![no_std]

pub mod bitmap;
pub mod controls;
pub mod ept;
pub mod event;
pub mod exit;
pub mod field;
pub mod vmcs;

pub use bitmap::{IoBitmaps, MsrBitmap};
pub use controls::Controls;
pub use ept::{Access, EptEntry, MemoryType};
pub use event::{Event, EventType};
pub use exit::{BasicExit, ExitReason};
pub use field::Field;
pub use vmcs::Basic;

/// CPUID leaf 1: ECX bit 5 is VMX.
pub const CPUID_FEATURES: u32 = 1;
/// CPUID.1:ECX.VMX: the CPU has VMX (SDM vol. 3C, 24.6).
pub const CPUID_1_ECX_VMX: u32 = 1 << 5;
/// CR4.VMXE: VMXON is allowed (and CR4.VMXE cannot be cleared in VMX
/// operation).
pub const CR4_VMXE: u64 = 1 << 13;

/// MSR IA32_FEATURE_CONTROL: the firmware's lock and VMX enables.
pub const MSR_FEATURE_CONTROL: u32 = 0x03A;
/// IA32_FEATURE_CONTROL.Lock: the MSR is read-only until reset. VMXON
/// faults while it is clear.
pub const FEATURE_CONTROL_LOCK: u64 = 1 << 0;
/// IA32_FEATURE_CONTROL: VMXON is allowed inside SMX operation.
pub const FEATURE_CONTROL_VMX_IN_SMX: u64 = 1 << 1;
/// IA32_FEATURE_CONTROL: VMXON is allowed outside SMX operation, as the
/// kernel runs it.
pub const FEATURE_CONTROL_VMX_OUTSIDE_SMX: u64 = 1 << 2;

/// The VMX capability MSRs (SDM vol. 3D, appendix A).
pub mod msr {
    pub const BASIC: u32 = 0x480;
    pub const PINBASED_CTLS: u32 = 0x481;
    pub const PROCBASED_CTLS: u32 = 0x482;
    pub const EXIT_CTLS: u32 = 0x483;
    pub const ENTRY_CTLS: u32 = 0x484;
    pub const MISC: u32 = 0x485;
    pub const CR0_FIXED0: u32 = 0x486;
    pub const CR0_FIXED1: u32 = 0x487;
    pub const CR4_FIXED0: u32 = 0x488;
    pub const CR4_FIXED1: u32 = 0x489;
    pub const VMCS_ENUM: u32 = 0x48A;
    pub const PROCBASED_CTLS2: u32 = 0x48B;
    pub const EPT_VPID_CAP: u32 = 0x48C;
    pub const TRUE_PINBASED_CTLS: u32 = 0x48D;
    pub const TRUE_PROCBASED_CTLS: u32 = 0x48E;
    pub const TRUE_EXIT_CTLS: u32 = 0x48F;
    pub const TRUE_ENTRY_CTLS: u32 = 0x490;
    pub const VMFUNC: u32 = 0x491;
    pub const PROCBASED_CTLS3: u32 = 0x492;
}

/// A 4 KiB page's size: the VMCS and VMXON regions, the MSR bitmap, each
/// I/O bitmap and each EPT table are one.
pub const PAGE: u64 = 4096;

#[cfg(test)]
mod tests;
