//! Per-CPU privilege setup for ring 3, run on every CPU before it schedules anything.
//!
//! Nothing here trusts what the firmware or Limine left behind: CR0 and CR4 are
//! rebuilt from known bits, and every MSR that controls a ring transition is written.
//! - SYSCALL enters at `entry::syscall_entry` with IF, TF, DF, NT, AC and IOPL
//!   cleared; SYSENTER is disabled (IA32_SYSENTER_CS = 0).
//! - x87, SSE and AVX are on for ring 3: CR0.EM and TS are clear and NE is set (x87
//!   errors raise #MF), CR4.OSFXSR, OSXMMEXCPT (SIMD errors raise #XM) and OSXSAVE
//!   are set, and XCR0 is x87|SSE|AVX, plus AVX-512's opmask and zmm state when the
//!   CPU can save them, as `cpufeat` chooses (see `fpu`, which saves the state at
//!   thread switches). The kernel itself stays soft-float.
//! - SMEP, SMAP and UMIP are turned on when the CPU has them. FSGSBASE, PCID, PKE,
//!   PCE and CET stay off: with FSGSBASE off, ring 3 cannot choose its GS base.

use crate::gdt::{KERNEL_CODE, KERNEL_DATA, USER_CODE, USER_DATA};
use crate::percpu;
use core::arch::x86_64::__cpuid_count;
use core::sync::atomic::{AtomicBool, Ordering};
use x86_64::registers::model_specific::{Msr, Star};
use x86_64::structures::gdt::SegmentSelector;

const CR0_MP: u64 = 1 << 1;
const CR0_EM: u64 = 1 << 2;
const CR0_TS: u64 = 1 << 3;
const CR0_NE: u64 = 1 << 5;

const CR4_PSE: u64 = 1 << 4;
const CR4_PAE: u64 = 1 << 5;
const CR4_MCE: u64 = 1 << 6;
const CR4_PGE: u64 = 1 << 7;
const CR4_OSFXSR: u64 = 1 << 9;
const CR4_OSXMMEXCPT: u64 = 1 << 10;
const CR4_UMIP: u64 = 1 << 11;
const CR4_LA57: u64 = 1 << 12;
const CR4_PCIDE: u64 = 1 << 17;
const CR4_OSXSAVE: u64 = 1 << 18;
const CR4_SMEP: u64 = 1 << 20;
const CR4_SMAP: u64 = 1 << 21;
/// The only bits kept from whatever was there before.
const CR4_KEEP: u64 = CR4_PSE | CR4_PAE | CR4_MCE | CR4_PGE;

const EFER_SCE: u64 = 1 << 0;

const MSR_EFER: u32 = 0xC000_0080;
const MSR_LSTAR: u32 = 0xC000_0082;
const MSR_CSTAR: u32 = 0xC000_0083;
const MSR_SFMASK: u32 = 0xC000_0084;
const MSR_FS_BASE: u32 = 0xC000_0100;
const MSR_KERNEL_GS_BASE: u32 = 0xC000_0102;
const MSR_SYSENTER_CS: u32 = 0x174;

/// RFLAGS bits SYSCALL clears: TF, IF, DF, IOPL, NT, AC.
const SFMASK: u64 = (1 << 8) | (1 << 9) | (1 << 10) | (3 << 12) | (1 << 14) | (1 << 18);

/// What the boot CPU found; every other CPU must find the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features {
    pub smep: bool,
    pub smap: bool,
    pub umip: bool,
}

static SMAP_ON: AtomicBool = AtomicBool::new(false);

pub fn features() -> Features {
    // An unsupported leaf reads as zeros, meaning none of the features.
    let leaf7 = __cpuid_count(7, 0);
    Features {
        smep: leaf7.ebx & (1 << 7) != 0,
        smap: leaf7.ebx & (1 << 20) != 0,
        umip: leaf7.ecx & (1 << 2) != 0,
    }
}

fn read_cr0() -> u64 {
    let value: u64;
    // SAFETY: reading CR0 has no side effects.
    unsafe {
        core::arch::asm!("mov {}, cr0", out(reg) value, options(nomem, nostack, preserves_flags))
    };
    value
}

pub fn read_cr4() -> u64 {
    let value: u64;
    // SAFETY: reading CR4 has no side effects.
    unsafe {
        core::arch::asm!("mov {}, cr4", out(reg) value, options(nomem, nostack, preserves_flags))
    };
    value
}

/// Sets up this CPU for ring 3. `syscall_entry` is the SYSCALL target.
pub fn init(syscall_entry: u64) -> Result<Features, &'static str> {
    let features = features();
    let vector = cpufeat::Features::detect();
    if !vector.xsave {
        return Err("cpu: no XSAVE, which ring 3's x87, SSE and AVX state needs");
    }
    let old_cr4 = read_cr4();
    if old_cr4 & CR4_LA57 != 0 {
        return Err("cpu: 5-level paging is on; the kernel assumes 4 levels");
    }
    let mut cr4 = (old_cr4 & CR4_KEEP) | CR4_OSFXSR | CR4_OSXMMEXCPT | CR4_OSXSAVE;
    if features.smep {
        cr4 |= CR4_SMEP;
    }
    if features.smap {
        cr4 |= CR4_SMAP;
    }
    if features.umip {
        cr4 |= CR4_UMIP;
    }
    debug_assert_eq!(cr4 & CR4_PCIDE, 0);
    let cr0 = (read_cr0() | CR0_MP | CR0_NE) & !(CR0_EM | CR0_TS);
    // SAFETY: the kernel is soft-float and never touches user memory directly, so
    // turning on the FPU, SSE and XSAVE (which the CPU has) and SMEP/SMAP/UMIP cannot
    // break kernel code; PAE, PGE and paging stay as they are.
    unsafe {
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
        let mut efer = Msr::new(MSR_EFER);
        efer.write(efer.read() | EFER_SCE);
        Msr::new(MSR_LSTAR).write(syscall_entry);
        // No 32-bit code segment exists, so compatibility mode is unreachable;
        // CSTAR still points at the 64-bit entry rather than at nothing.
        Msr::new(MSR_CSTAR).write(syscall_entry);
        Msr::new(MSR_SFMASK).write(SFMASK);
        Msr::new(MSR_SYSENTER_CS).write(0);
        Msr::new(MSR_FS_BASE).write(0);
        Msr::new(MSR_KERNEL_GS_BASE).write(0);
        core::arch::asm!("lldt {0:x}", in(reg) 0u16, options(nostack, preserves_flags));
    }
    crate::fpu::init_cpu(vector.xcr0_supported)?;
    Star::write(
        SegmentSelector(USER_CODE),
        SegmentSelector(USER_DATA),
        SegmentSelector(KERNEL_CODE),
        SegmentSelector(KERNEL_DATA),
    )
    .map_err(|_| "cpu: GDT layout does not fit SYSCALL")?;
    let cpu = percpu::this();
    cpu.cr0.store(read_cr0(), Ordering::Relaxed);
    cpu.cr4.store(read_cr4(), Ordering::Relaxed);
    // SAFETY: reading EFER has no side effects.
    cpu.efer
        .store(unsafe { Msr::new(MSR_EFER).read() }, Ordering::Relaxed);
    // Read back with XGETBV, so the boot check sees what the CPU really has.
    cpu.xcr0
        .store(cpufeat::Features::detect().xcr0, Ordering::Relaxed);
    SMAP_ON.store(features.smap, Ordering::Relaxed);
    Ok(features)
}

/// Clears RFLAGS.AC, which ring 3 can set with `popf`, so SMAP holds in the kernel.
pub fn clac() {
    if SMAP_ON.load(Ordering::Relaxed) {
        // SAFETY: CLAC exists when SMAP does, and only clears RFLAGS.AC.
        unsafe { core::arch::asm!("clac", options(nomem, nostack)) };
    }
}

/// Boot check: every online CPU ended up with the same CR0, CR4, EFER and XCR0.
pub fn same_on_every_cpu(online: usize) -> Result<(), &'static str> {
    let first = percpu::get(0);
    let values = |cpu: &percpu::PerCpu| {
        (
            cpu.cr0.load(Ordering::Relaxed),
            cpu.cr4.load(Ordering::Relaxed),
            cpu.efer.load(Ordering::Relaxed),
            cpu.xcr0.load(Ordering::Relaxed),
        )
    };
    if (1..online).all(|i| values(percpu::get(i)) == values(first)) {
        Ok(())
    } else {
        Err("cpu: CR0, CR4, EFER or XCR0 differ between CPUs")
    }
}
