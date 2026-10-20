//! What the CPU has (CPUID) and what the OS lets programs use (XGETBV).
//!
//! An instruction set is usable only when the CPU has it and the OS saves the
//! registers it needs: AVX needs XCR0's SSE and AVX bits, AVX-512 also its opmask
//! and ZMM bits. Reading XCR0 needs CR4.OSXSAVE, so [`Features::detect`] reads it
//! only when CPUID says OSXSAVE is set, and works in ring 0 and ring 3 alike.

#![no_std]

/// XCR0 bits for the x87, SSE, AVX (upper ymm), opmask and two ZMM state components.
pub const XCR0_X87: u64 = 1 << 0;
pub const XCR0_SSE: u64 = 1 << 1;
pub const XCR0_AVX: u64 = 1 << 2;
pub const XCR0_OPMASK: u64 = 1 << 5;
pub const XCR0_ZMM_HI256: u64 = 1 << 6;
pub const XCR0_HI16_ZMM: u64 = 1 << 7;
const YMM_STATE: u64 = XCR0_SSE | XCR0_AVX;
const ZMM_STATE: u64 = YMM_STATE | XCR0_OPMASK | XCR0_ZMM_HI256 | XCR0_HI16_ZMM;

/// CPUID bits and XCR0 as read. Fields are what CPUID reports; use the methods to
/// ask what may run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Features {
    pub sse2: bool,
    pub avx: bool,
    pub avx2: bool,
    pub fma: bool,
    pub f16c: bool,
    pub avx512f: bool,
    pub avx512bw: bool,
    pub avx512vnni: bool,
    pub xsave: bool,
    /// CR4.OSXSAVE as CPUID reflects it: the OS has turned XSAVE on.
    pub osxsave: bool,
    /// Running under a hypervisor (CPUID.1:ECX bit 31).
    pub hypervisor: bool,
    /// XCR0, or 0 when the OS has not turned XSAVE on.
    pub xcr0: u64,
    /// The XCR0 bits the CPU supports (CPUID.(0xD,0) EDX:EAX).
    pub xcr0_supported: u64,
    /// Bytes of an XSAVE area for every supported component (CPUID.(0xD,0) ECX).
    pub xsave_size_max: u32,
}

impl Features {
    #[cfg(target_arch = "x86_64")]
    pub fn detect() -> Self {
        let leaf0 = cpuid(0, 0);
        let max_leaf = leaf0.0;
        let leaf1 = cpuid(1, 0);
        let leaf7 = if max_leaf >= 7 {
            cpuid(7, 0)
        } else {
            (0, 0, 0, 0)
        };
        let bit = |reg: u32, n: u32| reg & (1 << n) != 0;
        let (_, _, ecx1, edx1) = leaf1;
        let (_, ebx7, ecx7, _) = leaf7;
        let xsave = bit(ecx1, 26);
        let osxsave = bit(ecx1, 27);
        let (xcr0_supported, xsave_size_max) = if xsave && max_leaf >= 0xd {
            let (eax, _, ecx, edx) = cpuid(0xd, 0);
            ((u64::from(edx) << 32) | u64::from(eax), ecx)
        } else {
            (0, 0)
        };
        Self {
            sse2: bit(edx1, 26),
            avx: bit(ecx1, 28),
            avx2: bit(ebx7, 5),
            fma: bit(ecx1, 12),
            f16c: bit(ecx1, 29),
            avx512f: bit(ebx7, 16),
            avx512bw: bit(ebx7, 30),
            avx512vnni: bit(ecx7, 11),
            xsave,
            osxsave,
            hypervisor: bit(ecx1, 31),
            // SAFETY: OSXSAVE is set, so XGETBV is enabled.
            xcr0: if osxsave { unsafe { xgetbv0() } } else { 0 },
            xcr0_supported,
            xsave_size_max,
        }
    }

    /// Nothing is known off x86_64, so only baseline code runs.
    #[cfg(not(target_arch = "x86_64"))]
    pub fn detect() -> Self {
        Self::default()
    }

    /// The OS saves the full ymm registers.
    pub fn ymm_enabled(&self) -> bool {
        self.xcr0 & YMM_STATE == YMM_STATE
    }

    /// The OS saves the full zmm registers and the opmask registers.
    pub fn zmm_enabled(&self) -> bool {
        self.xcr0 & ZMM_STATE == ZMM_STATE
    }

    /// AVX2 and FMA may run.
    pub fn avx2_fma(&self) -> bool {
        self.avx && self.avx2 && self.fma && self.ymm_enabled()
    }

    /// AVX-512 F and BW may run.
    pub fn avx512(&self) -> bool {
        self.avx512f && self.avx512bw && self.zmm_enabled()
    }

    /// Names of the vector instruction sets that may run, for logs and `setup`.
    /// SSE2 is listed from CPUID alone: CR4.OSFXSR, which SSE also needs, cannot be
    /// seen from ring 3.
    pub fn usable(&self) -> impl Iterator<Item = &'static str> {
        let list = [
            ("sse2", self.sse2),
            ("avx", self.avx && self.ymm_enabled()),
            ("avx2", self.avx2 && self.ymm_enabled()),
            ("fma", self.fma && self.ymm_enabled()),
            ("f16c", self.f16c && self.ymm_enabled()),
            ("avx512f", self.avx512f && self.zmm_enabled()),
            ("avx512bw", self.avx512bw && self.zmm_enabled()),
            ("avx512vnni", self.avx512vnni && self.zmm_enabled()),
        ];
        list.into_iter().filter(|(_, on)| *on).map(|(name, _)| name)
    }
}

/// EAX, EBX, ECX and EDX of one CPUID leaf.
#[cfg(target_arch = "x86_64")]
fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    #[allow(unused_unsafe)]
    // SAFETY: CPUID exists on every x86_64 CPU and only reads.
    let r = unsafe { core::arch::x86_64::__cpuid_count(leaf, subleaf) };
    (r.eax, r.ebx, r.ecx, r.edx)
}

/// XCR0.
///
/// # Safety
///
/// CPUID.1:ECX.OSXSAVE must be set; otherwise XGETBV raises #UD.
#[cfg(target_arch = "x86_64")]
unsafe fn xgetbv0() -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: the caller checked OSXSAVE, so XGETBV runs in any ring; it reads
    // XCR0 and changes nothing.
    unsafe {
        core::arch::asm!("xgetbv", in("ecx") 0u32, out("eax") lo, out("edx") hi,
            options(nomem, nostack, preserves_flags));
    }
    (u64::from(hi) << 32) | u64::from(lo)
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::vec::Vec;

    #[test]
    fn usable_sets_need_the_os_to_save_their_registers() {
        let cpu = Features {
            sse2: true,
            avx: true,
            avx2: true,
            fma: true,
            avx512f: true,
            avx512bw: true,
            osxsave: true,
            xcr0: XCR0_X87 | XCR0_SSE,
            ..Features::default()
        };
        assert!(!cpu.avx2_fma());
        assert!(!cpu.avx512());
        assert_eq!(cpu.usable().collect::<Vec<_>>(), ["sse2"]);
        let ymm = Features {
            xcr0: XCR0_X87 | YMM_STATE,
            ..cpu
        };
        assert!(ymm.avx2_fma());
        assert!(!ymm.avx512());
        let zmm = Features {
            xcr0: XCR0_X87 | ZMM_STATE,
            ..cpu
        };
        assert!(zmm.avx512());
        assert_eq!(
            zmm.usable().collect::<Vec<_>>(),
            ["sse2", "avx", "avx2", "fma", "avx512f", "avx512bw"]
        );
        let no_fma = Features { fma: false, ..zmm };
        assert!(!no_fma.avx2_fma());
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn detect_agrees_with_std() {
        let cpu = Features::detect();
        assert!(cpu.sse2);
        assert_eq!(
            cpu.avx2_fma(),
            std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma")
        );
        assert_eq!(
            cpu.f16c && cpu.ymm_enabled(),
            std::is_x86_feature_detected!("f16c")
        );
        assert_eq!(
            cpu.avx512(),
            std::is_x86_feature_detected!("avx512f") && std::is_x86_feature_detected!("avx512bw")
        );
        if cpu.osxsave {
            assert!(cpu.xcr0 & XCR0_X87 != 0);
            assert_eq!(cpu.xcr0 & !cpu.xcr0_supported, 0);
            assert!(cpu.xsave_size_max >= 576);
        }
    }
}
