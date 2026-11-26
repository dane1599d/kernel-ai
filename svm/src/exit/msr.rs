//! The kernel's MSR allow-list (ADR 0006 item 3): which MSRs a guest's
//! RDMSR and WRMSR reach the kernel's emulation for. ECX is not in the VMCB,
//! so [`dispatch`](super::dispatch) answers an MSR exit with
//! [`Kernel::Msr`](super::Kernel::Msr) and [`msr`] then decides from the
//! guest's RCX: on the list, the kernel emulates it; off it, #GP(0), as a
//! CPU gives for an MSR it does not have.
//!
//! The list is what a 64-bit Linux guest touches on a machine with an x2APIC
//! and no PMU or MCE banks. It decides only who handles an access: each
//! emulation still checks the value written (EFER keeps SVME clear, for
//! one).

use super::{Action, Exit, Fault, Guest, Kernel, dispatch};

/// MSRs `first..=last` and the directions the kernel emulates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Allowed {
    pub first: u32,
    pub last: u32,
    pub read: bool,
    pub write: bool,
    pub name: &'static str,
}

const fn rw(first: u32, last: u32, name: &'static str) -> Allowed {
    Allowed {
        first,
        last,
        read: true,
        write: true,
        name,
    }
}

const fn ro(msr: u32, name: &'static str) -> Allowed {
    Allowed {
        first: msr,
        last: msr,
        read: true,
        write: false,
        name,
    }
}

/// The list, in MSR order.
pub const MSR_ALLOWED: &[Allowed] = &[
    rw(0x0000_0010, 0x0000_0010, "TSC"),
    rw(0x0000_001B, 0x0000_001B, "APIC_BASE"),
    ro(0x0000_00FE, "MTRRCAP"),
    rw(0x0000_0174, 0x0000_0176, "SYSENTER_CS/ESP/EIP"),
    ro(0x0000_0179, "MCG_CAP"),
    rw(0x0000_017A, 0x0000_017A, "MCG_STATUS"),
    ro(0x0000_01A0, "MISC_ENABLE"),
    rw(0x0000_0277, 0x0000_0277, "PAT"),
    rw(0x0000_02FF, 0x0000_02FF, "MTRR_DEF_TYPE"),
    rw(0x0000_06E0, 0x0000_06E0, "TSC_DEADLINE"),
    rw(0x0000_0800, 0x0000_08FF, "x2APIC"),
    rw(0xC000_0080, 0xC000_0080, "EFER"),
    rw(0xC000_0081, 0xC000_0084, "STAR/LSTAR/CSTAR/SFMASK"),
    rw(0xC000_0100, 0xC000_0102, "FS_BASE/GS_BASE/KERNEL_GS_BASE"),
    rw(0xC000_0103, 0xC000_0103, "TSC_AUX"),
];

/// Whether the kernel emulates a read of `msr`, or a write when `write`.
pub fn msr_allowed(msr: u32, write: bool) -> bool {
    MSR_ALLOWED
        .iter()
        .any(|a| (a.first..=a.last).contains(&msr) && if write { a.write } else { a.read })
}

/// [`dispatch`], with the MSR exit decided by the guest's `rcx`: emulated
/// as `dispatch` said when the MSR is on the list in that direction, else
/// #GP(0) with RIP unchanged. RDMSR and WRMSR read ECX, so RCX's upper half
/// is ignored. Every other exit is `dispatch`'s answer.
pub fn msr(exit: &Exit, guest: &Guest, rcx: u64) -> Action {
    match dispatch(exit, guest) {
        Action::Kernel(Kernel::Msr { write }, next) => match msr_allowed(rcx as u32, write) {
            true => Action::Kernel(Kernel::Msr { write }, next),
            false => match crate::Event::exception(13, Some(0)) {
                Ok(event) => Action::Inject(Fault { event, cr2: None }),
                Err(_) => Action::Kill(super::Kill::BadInfo(exit.code)),
            },
        },
        other => other,
    }
}
