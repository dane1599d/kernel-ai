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
/// The three AVX-512 components, which XSETBV takes only all together, with AVX.
pub const XCR0_AVX512: u64 = XCR0_OPMASK | XCR0_ZMM_HI256 | XCR0_HI16_ZMM;
const YMM_STATE: u64 = XCR0_SSE | XCR0_AVX;
const ZMM_STATE: u64 = YMM_STATE | XCR0_AVX512;
/// Bytes every XSAVE area starts with: the 512-byte legacy region (x87, MXCSR and
/// xmm) and the 64-byte header.
pub const XSAVE_LEGACY_AND_HEADER: usize = 576;

/// The XCR0 the kernel turns on for ring 3: x87 and SSE; AVX when the CPU can save
/// it; and AVX-512's opmask and zmm components when it can save all three, which
/// gives 0xE7. Every other component (MPX, PKRU, AMX) stays off. `supported` is
/// CPUID.(0xD,0) EDX:EAX; None when XSAVE cannot hold x87 and SSE state at all.
/// XSETBV faults on bits 5-7 set without AVX, or set only in part, so neither is
/// ever returned.
pub fn ring3_xcr0(supported: u64) -> Option<u64> {
    let base = XCR0_X87 | XCR0_SSE;
    if supported & base != base {
        return None;
    }
    let avx = supported & XCR0_AVX != 0;
    let avx512 = avx && supported & XCR0_AVX512 == XCR0_AVX512;
    Some(base | if avx { XCR0_AVX } else { 0 } | if avx512 { XCR0_AVX512 } else { 0 })
}

/// The XSAVE area size an OS may trust for `xcr0`: `reported` is CPUID.(0xD,0) EBX
/// read after XSETBV, `extended_end` is where the last enabled component above SSE
/// ends (the largest CPUID.(0xD,i) EBX + EAX over those components, 0 if none), and
/// `room` is what the OS reserved per thread. The area must hold the legacy region
/// and header and every enabled component, and fit in `room`.
pub fn xsave_area_bytes(
    xcr0: u64,
    reported: u32,
    extended_end: u32,
    room: usize,
) -> Result<usize, &'static str> {
    let size = usize::try_from(reported).map_err(|_| "cpu: XSAVE area size overflows")?;
    let end = usize::try_from(extended_end).map_err(|_| "cpu: XSAVE offset overflows")?;
    let extended = xcr0 & !(XCR0_X87 | XCR0_SSE) != 0;
    if size < XSAVE_LEGACY_AND_HEADER {
        Err("cpu: XSAVE area is smaller than its legacy region and header")
    } else if extended && (end <= XSAVE_LEGACY_AND_HEADER || size < end) {
        Err("cpu: XSAVE area does not hold every enabled component")
    } else if size > room {
        Err("cpu: XSAVE area is larger than the space reserved per thread")
    } else {
        Ok(size)
    }
}

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
    /// AVX-512 VL: the AVX-512 instructions on xmm and ymm registers too.
    pub avx512vl: bool,
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
    /// Bytes of an XSAVE area for the components XCR0 enables now (CPUID.(0xD,0) EBX).
    pub xsave_size: u32,
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
        let (xcr0_supported, xsave_size_max, xsave_size) = if xsave && max_leaf >= 0xd {
            let (eax, ebx, ecx, edx) = cpuid(0xd, 0);
            ((u64::from(edx) << 32) | u64::from(eax), ecx, ebx)
        } else {
            (0, 0, 0)
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
            avx512vl: bit(ebx7, 31),
            xsave,
            osxsave,
            hypervisor: bit(ecx1, 31),
            // SAFETY: OSXSAVE is set, so XGETBV is enabled.
            xcr0: if osxsave { unsafe { xgetbv0() } } else { 0 },
            xcr0_supported,
            xsave_size_max,
            xsave_size,
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
        let (ymm, zmm) = (self.ymm_enabled(), self.zmm_enabled());
        self.sets(ymm, zmm)
    }

    /// Names of the vector instruction sets CPUID reports, whether or not the OS
    /// saves their registers.
    pub fn offered(&self) -> impl Iterator<Item = &'static str> {
        self.sets(true, true)
    }

    fn sets(&self, ymm: bool, zmm: bool) -> impl Iterator<Item = &'static str> {
        let list = [
            ("sse2", self.sse2),
            ("avx", self.avx && ymm),
            ("avx2", self.avx2 && ymm),
            ("fma", self.fma && ymm),
            ("f16c", self.f16c && ymm),
            ("avx512f", self.avx512f && zmm),
            ("avx512bw", self.avx512bw && zmm),
            ("avx512vnni", self.avx512vnni && zmm),
        ];
        list.into_iter().filter(|(_, on)| *on).map(|(name, _)| name)
    }
}

/// The hypervisor's 12-byte vendor signature (CPUID leaf 0x40000000, EBX, ECX and
/// EDX), when CPUID says a hypervisor is present and it fills the leaf.
#[cfg(target_arch = "x86_64")]
pub fn hypervisor_signature() -> Option<[u8; 12]> {
    if cpuid(1, 0).2 & (1 << 31) == 0 {
        return None;
    }
    let (max_leaf, ebx, ecx, edx) = cpuid(0x4000_0000, 0);
    let mut signature = [0; 12];
    signature[..4].copy_from_slice(&ebx.to_le_bytes());
    signature[4..8].copy_from_slice(&ecx.to_le_bytes());
    signature[8..].copy_from_slice(&edx.to_le_bytes());
    (max_leaf >= 0x4000_0000 && signature != [0; 12]).then_some(signature)
}

#[cfg(not(target_arch = "x86_64"))]
pub fn hypervisor_signature() -> Option<[u8; 12]> {
    None
}

/// What a hypervisor signature names, in plain words.
pub fn hypervisor_name(signature: &[u8; 12]) -> &'static str {
    match signature {
        b"KVMKVMKVM\0\0\0" => "KVM",
        b"TCGTCGTCGTCG" => "QEMU emulating the CPU (TCG)",
        b"Microsoft Hv" => "Hyper-V",
        b"VMwareVMware" => "VMware",
        b"VBoxVBoxVBox" => "VirtualBox",
        b"XenVMMXenVMM" => "Xen",
        b" lrpepyh  vr" => "Parallels",
        b"bhyve bhyve " => "bhyve",
        _ => "a hypervisor this list does not name",
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

    #[test]
    fn ring3_gets_avx512_only_with_all_its_components_and_avx() {
        let x87_sse = XCR0_X87 | XCR0_SSE;
        // PKRU (bit 9) and MPX (bits 3-4) are never turned on.
        let everything = XCR0_X87 | ZMM_STATE | (1 << 9) | (3 << 3);
        assert_eq!(ring3_xcr0(everything), Some(0xE7));
        assert_eq!(ring3_xcr0(XCR0_X87 | YMM_STATE), Some(0x7));
        assert_eq!(ring3_xcr0(x87_sse), Some(x87_sse));
        // Only part of AVX-512's state, or AVX-512 without AVX: no AVX-512.
        let part = XCR0_X87 | YMM_STATE | XCR0_OPMASK | XCR0_ZMM_HI256;
        assert_eq!(ring3_xcr0(part), Some(0x7));
        assert_eq!(ring3_xcr0(x87_sse | XCR0_AVX512), Some(x87_sse));
        assert_eq!(ring3_xcr0(XCR0_X87 | XCR0_AVX), None);
        assert_eq!(ring3_xcr0(XCR0_SSE), None);
        assert_eq!(ring3_xcr0(0), None);
    }

    #[test]
    fn xsave_sizes_are_checked_before_use() {
        let zmm = 0xE7;
        let ymm = XCR0_X87 | YMM_STATE;
        let xmm = XCR0_X87 | XCR0_SSE;
        assert_eq!(xsave_area_bytes(ymm, 832, 832, 4096), Ok(832));
        assert_eq!(xsave_area_bytes(xmm, 576, 0, 4096), Ok(576));
        // AMD Zen 4 (the 7700X, EPYC 9V74): Hi16_ZMM ends at 1408 + 1024.
        assert_eq!(xsave_area_bytes(zmm, 2432, 2432, 4096), Ok(2432));
        // Intel leaves MPX's 128 bytes in the standard layout: it ends at 2688.
        assert_eq!(xsave_area_bytes(zmm, 2688, 2688, 4096), Ok(2688));
        // A reported size short of the last component is refused.
        assert!(xsave_area_bytes(zmm, 2432, 2688, 4096).is_err());
        assert!(xsave_area_bytes(zmm, 832, 0, 4096).is_err());
        assert!(xsave_area_bytes(xmm, 512, 0, 4096).is_err());
        assert!(xsave_area_bytes(ymm, 576, 832, 4096).is_err());
        assert!(xsave_area_bytes(ymm, 832, 0, 4096).is_err());
        assert!(xsave_area_bytes(ymm, 8192, 832, 4096).is_err());
        assert!(xsave_area_bytes(ymm, 4097, 832, 4096).is_err());
    }

    #[test]
    fn offered_lists_cpuid_even_when_the_os_saves_nothing() {
        let cpu = Features {
            sse2: true,
            avx2: true,
            avx512f: true,
            xcr0: XCR0_X87 | XCR0_SSE,
            ..Features::default()
        };
        assert_eq!(
            cpu.offered().collect::<Vec<_>>(),
            ["sse2", "avx2", "avx512f"]
        );
        assert_eq!(cpu.usable().collect::<Vec<_>>(), ["sse2"]);
    }

    #[test]
    fn hypervisors_are_named_from_their_signature() {
        assert_eq!(hypervisor_name(b"KVMKVMKVM\0\0\0"), "KVM");
        assert_eq!(hypervisor_name(b"Microsoft Hv"), "Hyper-V");
        assert!(hypervisor_name(b"TCGTCGTCGTCG").contains("TCG"));
        assert_eq!(
            hypervisor_name(b"KVMKVMKVMKVM"),
            "a hypervisor this list does not name"
        );
        // The signature is read only when CPUID says there is a hypervisor.
        if let Some(signature) = hypervisor_signature() {
            assert_ne!(signature, [0; 12]);
            assert!(Features::detect().hypervisor);
        }
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
        if cpu.avx512() {
            assert_eq!(cpu.avx512vl, std::is_x86_feature_detected!("avx512vl"));
            assert_eq!(cpu.avx512vnni, std::is_x86_feature_detected!("avx512vnni"));
        }
        if cpu.osxsave {
            assert!(cpu.xcr0 & XCR0_X87 != 0);
            assert_eq!(cpu.xcr0 & !cpu.xcr0_supported, 0);
            assert!(cpu.xsave_size_max as usize >= XSAVE_LEGACY_AND_HEADER);
            assert!(cpu.xsave_size as usize >= XSAVE_LEGACY_AND_HEADER);
            assert!(cpu.xsave_size <= cpu.xsave_size_max);
        }
    }
}
