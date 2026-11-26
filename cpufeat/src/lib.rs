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

/// EFER.NXE: page-table entries may forbid execution. W^X (ADR 0012 D5, D9)
/// rests on it, and only the boot loader sets it, so the kernel checks it.
pub const EFER_NXE: u64 = 1 << 11;

/// Whether EFER, as read, lets pages forbid execution; why not, if it does not.
pub fn nx_enabled(efer: u64) -> Result<(), &'static str> {
    if efer & EFER_NXE != 0 {
        Ok(())
    } else {
        Err("cpu: EFER.NXE is clear, so no page can forbid execution (W^X)")
    }
}
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
    /// AMD SVM and its extensions, when CPUID offers SVM (ADR 0006 step 1).
    pub svm: Option<Svm>,
}

/// What AMD's secure virtual machine offers (CPUID 0x8000_0001 ECX bit 2 and leaf
/// 0x8000_000A; AMD64 APM vol. 3, appendix E). Firmware can still have disabled
/// SVM through VM_CR.SVMDIS, which only ring 0 can read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Svm {
    /// The SVM revision (EAX bits 7:0).
    pub revision: u8,
    /// How many address-space ids the CPU tags TLB entries with (EBX).
    pub asids: u32,
    /// Nested paging (EDX bit 0).
    pub npt: bool,
    /// The next-RIP save on #VMEXIT (EDX bit 3).
    pub nrips: bool,
    /// Decode assists: the faulting instruction's bytes on #VMEXIT (EDX bit 7).
    pub decode_assists: bool,
    /// The advanced virtual interrupt controller (EDX bit 13).
    pub avic: bool,
}

const SVM_LEAF: u32 = 0x8000_000A;

impl Svm {
    /// Decodes SVM from the highest extended leaf, CPUID 0x8000_0001 ECX, and leaf
    /// 0x8000_000A's EAX, EBX and EDX. None without the SVM bit; a CPU that sets
    /// the bit but has no leaf 0x8000_000A reports SVM with no extensions.
    pub fn decode(max_extended: u32, ecx_8000_0001: u32, leaf_a: (u32, u32, u32)) -> Option<Self> {
        if ecx_8000_0001 & (1 << 2) == 0 {
            return None;
        }
        if max_extended < SVM_LEAF {
            return Some(Self::default());
        }
        let (eax, ebx, edx) = leaf_a;
        let bit = |n: u32| edx & (1 << n) != 0;
        Some(Self {
            revision: eax as u8,
            asids: ebx,
            npt: bit(0),
            nrips: bit(3),
            decode_assists: bit(7),
            avic: bit(13),
        })
    }

    /// The boot line's words: `SVM rev 1, 32768 ASIDs, npt nrips decode-assists avic`.
    pub fn describe(&self) -> SvmLine {
        SvmLine(*self)
    }
}

/// [`Svm`] as the boot line prints it.
pub struct SvmLine(Svm);

impl core::fmt::Display for SvmLine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let svm = &self.0;
        write!(f, "SVM rev {}, {} ASIDs,", svm.revision, svm.asids)?;
        let parts = [
            ("npt", svm.npt),
            ("nrips", svm.nrips),
            ("decode-assists", svm.decode_assists),
            ("avic", svm.avic),
        ];
        let mut any = false;
        for (name, on) in parts {
            if on {
                write!(f, " {name}")?;
                any = true;
            }
        }
        if !any {
            f.write_str(" no extensions")?;
        }
        Ok(())
    }
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
        let max_extended = cpuid(0x8000_0000, 0).0;
        let ecx_ext = if max_extended >= 0x8000_0001 {
            cpuid(0x8000_0001, 0).2
        } else {
            0
        };
        let leaf_a = if max_extended >= SVM_LEAF {
            let (eax, ebx, _, edx) = cpuid(SVM_LEAF, 0);
            (eax, ebx, edx)
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
            svm: Svm::decode(max_extended, ecx_ext, leaf_a),
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

/// The CPU vendor's 12-byte signature (CPUID leaf 0, EBX, EDX and ECX), such as
/// `AuthenticAMD` or `GenuineIntel`; under a hypervisor, what it chose to show.
#[cfg(target_arch = "x86_64")]
pub fn vendor() -> [u8; 12] {
    let (_, ebx, ecx, edx) = cpuid(0, 0);
    let mut vendor = [0; 12];
    vendor[..4].copy_from_slice(&ebx.to_le_bytes());
    vendor[4..8].copy_from_slice(&edx.to_le_bytes());
    vendor[8..].copy_from_slice(&ecx.to_le_bytes());
    vendor
}

#[cfg(not(target_arch = "x86_64"))]
pub fn vendor() -> [u8; 12] {
    [0; 12]
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

/// Spike S06-1's nesting census (ADR 0006): the raw virtualisation bits CPUID
/// shows this guest, for the boot line `nesting: ...`. VMX is CPUID.1:ECX bit 5;
/// SVM is 0x8000_0001 ECX bit 2; leaf 0x8000_000A's four registers are kept raw
/// so the census can read bits [`Svm`] does not name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Nesting {
    pub vmx: bool,
    pub max_extended: u32,
    /// Leaf 0x8000_000A's EAX, EBX, ECX and EDX, or zeros when the CPU has no
    /// such leaf.
    pub leaf_a: (u32, u32, u32, u32),
    pub svm: Option<Svm>,
}

impl Nesting {
    /// Decodes the census from CPUID.1 ECX, the highest extended leaf, 0x8000_0001
    /// ECX and leaf 0x8000_000A's registers.
    pub fn decode(
        ecx_1: u32,
        max_extended: u32,
        ecx_8000_0001: u32,
        leaf_a: (u32, u32, u32, u32),
    ) -> Self {
        let leaf_a = if max_extended >= SVM_LEAF {
            leaf_a
        } else {
            (0, 0, 0, 0)
        };
        let (eax, ebx, _, edx) = leaf_a;
        Self {
            vmx: ecx_1 & (1 << 5) != 0,
            max_extended,
            leaf_a,
            svm: Svm::decode(max_extended, ecx_8000_0001, (eax, ebx, edx)),
        }
    }

    #[cfg(target_arch = "x86_64")]
    pub fn detect() -> Self {
        let ecx_1 = cpuid(1, 0).2;
        let max_extended = cpuid(0x8000_0000, 0).0;
        let ecx_ext = if max_extended >= 0x8000_0001 {
            cpuid(0x8000_0001, 0).2
        } else {
            0
        };
        let leaf_a = if max_extended >= SVM_LEAF {
            cpuid(SVM_LEAF, 0)
        } else {
            (0, 0, 0, 0)
        };
        Self::decode(ecx_1, max_extended, ecx_ext, leaf_a)
    }

    #[cfg(not(target_arch = "x86_64"))]
    pub fn detect() -> Self {
        Self::default()
    }
}

/// `vmx=0 svm=1 npt=1 nrips=0 decode_assists=0 avic=0 asids=16 leaf_8000000a=eax:0x1,ebx:0x10,ecx:0x0,edx:0x1`:
/// one `key=value` word each, so a CI job can grep them.
impl core::fmt::Display for Nesting {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let svm = self.svm.unwrap_or_default();
        let on = |b: bool| u8::from(b);
        let (eax, ebx, ecx, edx) = self.leaf_a;
        write!(
            f,
            "vmx={} svm={} npt={} nrips={} decode_assists={} avic={} asids={} \
             leaf_8000000a=eax:{eax:#x},ebx:{ebx:#x},ecx:{ecx:#x},edx:{edx:#x}",
            on(self.vmx),
            on(self.svm.is_some()),
            on(svm.npt),
            on(svm.nrips),
            on(svm.decode_assists),
            on(svm.avic),
            svm.asids,
        )
    }
}

/// Spike S06-1's second step (groundwork G16): Intel's VMX capability MSRs, which
/// only ring 0 can read (Intel SDM vol. 3D, appendix A). The kernel reads them
/// when CPUID offers VMX, and each later MSR only when the earlier one says it
/// exists, so `None` means "not offered", not "not read".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VmxCaps {
    /// IA32_FEATURE_CONTROL (0x3A).
    pub feature_control: u64,
    /// IA32_VMX_BASIC (0x480).
    pub basic: u64,
    /// IA32_VMX_PROCBASED_CTLS (0x482).
    pub procbased: u64,
    /// IA32_VMX_PROCBASED_CTLS2 (0x48B), read when PROCBASED_CTLS allows bit 31,
    /// "activate secondary controls".
    pub procbased2: Option<u64>,
    /// IA32_VMX_EPT_VPID_CAP (0x48C), read when the secondary controls allow EPT
    /// or VPID.
    pub ept_vpid_cap: Option<u64>,
}

pub const IA32_FEATURE_CONTROL: u32 = 0x3A;
pub const IA32_VMX_BASIC: u32 = 0x480;
pub const IA32_VMX_PROCBASED_CTLS: u32 = 0x482;
pub const IA32_VMX_PROCBASED_CTLS2: u32 = 0x48B;
pub const IA32_VMX_EPT_VPID_CAP: u32 = 0x48C;

impl VmxCaps {
    /// Reads the MSRs through `rdmsr`, which the caller supplies (ring 0 only).
    /// Each MSR is read only when the one before it says it exists.
    pub fn read(mut rdmsr: impl FnMut(u32) -> u64) -> Self {
        let feature_control = rdmsr(IA32_FEATURE_CONTROL);
        let basic = rdmsr(IA32_VMX_BASIC);
        let procbased = rdmsr(IA32_VMX_PROCBASED_CTLS);
        let procbased2 = (procbased & (1 << 63) != 0).then(|| rdmsr(IA32_VMX_PROCBASED_CTLS2));
        let ept_vpid_cap = procbased2
            .filter(|c| c & ((1 << (32 + 1)) | (1 << (32 + 5))) != 0)
            .map(|_| rdmsr(IA32_VMX_EPT_VPID_CAP));
        Self {
            feature_control,
            basic,
            procbased,
            procbased2,
            ept_vpid_cap,
        }
    }

    /// Whether a secondary control may be set (its allowed-1 bit, the MSR's high
    /// half).
    fn secondary(&self, bit: u32) -> bool {
        self.procbased2.is_some_and(|c| c & (1 << (32 + bit)) != 0)
    }

    fn cap(&self, bit: u32) -> bool {
        self.ept_vpid_cap.is_some_and(|c| c & (1 << bit) != 0)
    }

    pub fn ept(&self) -> bool {
        self.secondary(1)
    }
    pub fn vpid(&self) -> bool {
        self.secondary(5)
    }
    pub fn unrestricted_guest(&self) -> bool {
        self.secondary(7)
    }
    /// VMXON is allowed outside SMX: FEATURE_CONTROL locked (bit 0) with bit 2
    /// set. A hypervisor's guest sees what its host chose to show.
    pub fn vmxon_allowed(&self) -> bool {
        self.feature_control & 0b101 == 0b101
    }
}

/// `vmxon_ok=1 ept=1 vpid=1 unrestricted=1 ept_4level=1 ept_wb=1 ept_2m=1 ept_1g=1
/// ept_ad=1 true_ctls=1 vmcs_rev=0x4 feature_control=0x5 basic=... procbased=...
/// procbased2=... ept_vpid_cap=...`: decoded words first, then the raw MSRs.
impl core::fmt::Display for VmxCaps {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let on = |b: bool| u8::from(b);
        let raw = |v: Option<u64>| v.unwrap_or(0);
        write!(
            f,
            "vmxon_ok={} ept={} vpid={} unrestricted={} ept_4level={} ept_wb={} ept_2m={} \
             ept_1g={} ept_ad={} true_ctls={} vmcs_rev={:#x} feature_control={:#x} \
             basic={:#x} procbased={:#x} procbased2={:#x} ept_vpid_cap={:#x}",
            on(self.vmxon_allowed()),
            on(self.ept()),
            on(self.vpid()),
            on(self.unrestricted_guest()),
            on(self.cap(6)),
            on(self.cap(14)),
            on(self.cap(16)),
            on(self.cap(17)),
            on(self.cap(21)),
            on(self.basic & (1 << 55) != 0),
            self.basic & 0x7fff_ffff,
            self.feature_control,
            self.basic,
            self.procbased,
            raw(self.procbased2),
            raw(self.ept_vpid_cap),
        )
    }
}

/// The VMX control capability MSRs (Intel SDM vol. 3D, A.3-A.5), raw: each holds
/// the allowed-0 settings in its low half and the allowed-1 settings in its high
/// half. The TRUE ones are read only when IA32_VMX_BASIC bit 55 says they exist,
/// and PROCBASED_CTLS2 only when PROCBASED_CTLS allows secondary controls. For
/// round 21's `vmx::Controls::adjust()` test (G16's second read).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VmxCtls {
    pub pinbased: u64,
    pub procbased: u64,
    pub exit: u64,
    pub entry: u64,
    pub procbased2: Option<u64>,
    /// TRUE_PINBASED, TRUE_PROCBASED, TRUE_EXIT and TRUE_ENTRY (0x48D-0x490).
    pub true_ctls: Option<[u64; 4]>,
}

pub const IA32_VMX_PINBASED_CTLS: u32 = 0x481;
pub const IA32_VMX_EXIT_CTLS: u32 = 0x483;
pub const IA32_VMX_ENTRY_CTLS: u32 = 0x484;
pub const IA32_VMX_TRUE_PINBASED_CTLS: u32 = 0x48D;

impl VmxCtls {
    /// Reads the control MSRs through `rdmsr` (ring 0 only), given VMX_BASIC.
    pub fn read(basic: u64, mut rdmsr: impl FnMut(u32) -> u64) -> Self {
        let procbased = rdmsr(IA32_VMX_PROCBASED_CTLS);
        Self {
            pinbased: rdmsr(IA32_VMX_PINBASED_CTLS),
            procbased,
            exit: rdmsr(IA32_VMX_EXIT_CTLS),
            entry: rdmsr(IA32_VMX_ENTRY_CTLS),
            procbased2: (procbased & (1 << 63) != 0).then(|| rdmsr(IA32_VMX_PROCBASED_CTLS2)),
            true_ctls: (basic & (1 << 55) != 0)
                .then(|| core::array::from_fn(|i| rdmsr(IA32_VMX_TRUE_PINBASED_CTLS + i as u32))),
        }
    }
}

/// `pin=0x.. proc=0x.. exit=0x.. entry=0x.. proc2=0x.. true_pin=0x.. true_proc=0x..
/// true_exit=0x.. true_entry=0x..`; an MSR that does not exist prints `none`.
impl core::fmt::Display for VmxCtls {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "pin={:#x} proc={:#x} exit={:#x} entry={:#x}",
            self.pinbased, self.procbased, self.exit, self.entry
        )?;
        match self.procbased2 {
            Some(v) => write!(f, " proc2={v:#x}")?,
            None => f.write_str(" proc2=none")?,
        }
        let names = ["true_pin", "true_proc", "true_exit", "true_entry"];
        for (i, name) in names.iter().enumerate() {
            match self.true_ctls {
                Some(v) => write!(f, " {name}={:#x}", v[i])?,
                None => write!(f, " {name}=none")?,
            }
        }
        Ok(())
    }
}

/// Whether CPUID says the TSC runs at a constant rate in every power state
/// (0x80000007 EDX bit 8, "invariant TSC").
#[cfg(target_arch = "x86_64")]
pub fn invariant_tsc() -> bool {
    cpuid(0x8000_0000, 0).0 >= 0x8000_0007 && cpuid(0x8000_0007, 0).3 & (1 << 8) != 0
}

#[cfg(not(target_arch = "x86_64"))]
pub fn invariant_tsc() -> bool {
    false
}

/// What the kernel's clock is when the command line names none: the TSC is
/// tried only under KVM, whose every HPET read exits to the host, and only when
/// the TSC can be trusted not to drift; everywhere else the HPET. Each with the
/// reason, for the boot log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefaultClock {
    /// Try the TSC, calibrated against the HPET.
    Tsc(&'static str),
    Hpet(&'static str),
}

/// How far a second measured TSC rate `check` is from the first, `rate`, in
/// parts per million: the kernel's drift check after calibrating.
pub fn drift_ppm(rate: u64, check: u64) -> u64 {
    (u128::from(rate.abs_diff(check)) * 1_000_000 / u128::from(rate.max(1))) as u64
}

/// [`DefaultClock`] from the hypervisor's signature, CPUID's invariant-TSC bit
/// and the TSC rate the hypervisor reports.
pub fn default_clock(
    signature: Option<[u8; 12]>,
    invariant: bool,
    reported_khz: Option<u32>,
) -> DefaultClock {
    match signature {
        None => DefaultClock::Hpet("no hypervisor"),
        Some(sig) if &sig == b"KVMKVMKVM\0\0\0" => match (invariant, reported_khz) {
            (true, _) => DefaultClock::Tsc("KVM, invariant TSC"),
            (false, Some(_)) => DefaultClock::Tsc("KVM, the hypervisor reports the TSC's rate"),
            (false, None) => DefaultClock::Hpet("KVM, but no invariant or reported TSC rate"),
        },
        Some(sig) if &sig == b"TCGTCGTCGTCG" => DefaultClock::Hpet("TCG, which emulates the TSC"),
        Some(_) => DefaultClock::Hpet("a hypervisor other than KVM; clock=tsc tries the TSC"),
    }
}

/// The TSC's rate in kHz as the hypervisor reports it (CPUID 0x40000010, the
/// timing leaf VMware and VirtualBox fill), when a hypervisor offers that leaf.
#[cfg(target_arch = "x86_64")]
pub fn hypervisor_tsc_khz() -> Option<u32> {
    if cpuid(1, 0).2 & (1 << 31) == 0 || cpuid(0x4000_0000, 0).0 < 0x4000_0010 {
        return None;
    }
    let khz = cpuid(0x4000_0010, 0).0;
    (khz != 0).then_some(khz)
}

#[cfg(not(target_arch = "x86_64"))]
pub fn hypervisor_tsc_khz() -> Option<u32> {
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
    #[test]
    fn vmx_caps_read_each_msr_only_when_the_one_before_offers_it() {
        use std::collections::BTreeMap;
        let mut msrs = BTreeMap::from([
            (IA32_FEATURE_CONTROL, 0x5),
            (IA32_VMX_BASIC, (1 << 55) | (6 << 50) | (0x1000 << 32) | 4),
            (IA32_VMX_PROCBASED_CTLS, 1 << 63),
            (IA32_VMX_PROCBASED_CTLS2, (1 << 33) | (1 << 37) | (1 << 39)),
            (
                IA32_VMX_EPT_VPID_CAP,
                (1 << 6) | (1 << 14) | (1 << 16) | (1 << 17) | (1 << 21),
            ),
        ]);
        let caps = VmxCaps::read(|m| msrs[&m]);
        assert!(caps.vmxon_allowed() && caps.ept() && caps.vpid() && caps.unrestricted_guest());
        assert!(std::format!("{caps}").starts_with(
            "vmxon_ok=1 ept=1 vpid=1 unrestricted=1 ept_4level=1 ept_wb=1 ept_2m=1 ept_1g=1 \
             ept_ad=1 true_ctls=1 vmcs_rev=0x4 "
        ));

        // No secondary controls: neither later MSR is read.
        msrs.insert(IA32_VMX_PROCBASED_CTLS, 0);
        let mut read = Vec::new();
        let caps = VmxCaps::read(|m| {
            read.push(m);
            msrs[&m]
        });
        assert_eq!(
            read,
            [
                IA32_FEATURE_CONTROL,
                IA32_VMX_BASIC,
                IA32_VMX_PROCBASED_CTLS
            ]
        );
        assert!(!caps.ept() && !caps.unrestricted_guest() && caps.ept_vpid_cap.is_none());

        // Secondary controls without EPT or VPID: EPT_VPID_CAP is not read.
        msrs.insert(IA32_VMX_PROCBASED_CTLS, 1 << 63);
        msrs.insert(IA32_VMX_PROCBASED_CTLS2, 1 << 39);
        let caps = VmxCaps::read(|m| msrs[&m]);
        assert!(caps.unrestricted_guest() && !caps.ept() && caps.ept_vpid_cap.is_none());
    }

    #[test]
    fn vmx_ctls_read_true_msrs_only_with_basic_bit_55() {
        let mut read = Vec::new();
        let ctls = VmxCtls::read(1 << 55, |m| {
            read.push(m);
            u64::from(m)
        });
        assert_eq!(
            read,
            [0x482, 0x481, 0x483, 0x484, 0x48D, 0x48E, 0x48F, 0x490]
        );
        assert_eq!(
            std::format!("{ctls}"),
            "pin=0x481 proc=0x482 exit=0x483 entry=0x484 proc2=none \
             true_pin=0x48d true_proc=0x48e true_exit=0x48f true_entry=0x490"
        );
        let ctls = VmxCtls::read(0, |m| if m == 0x482 { 1 << 63 } else { u64::from(m) });
        assert_eq!(ctls.procbased2, Some(0x48B));
        assert!(ctls.true_ctls.is_none());
        assert!(std::format!("{ctls}").ends_with("true_entry=none"));
    }

    #[test]
    fn nesting_reads_vmx_svm_and_leaf_a() {
        // QEMU TCG's SVM: npt set, nrips clear (ADR 0006).
        let n = Nesting::decode(0, 0x8000_0021, 1 << 2, (1, 16, 0, 1));
        assert_eq!(
            std::format!("{n}"),
            "vmx=0 svm=1 npt=1 nrips=0 decode_assists=0 avic=0 asids=16 \
             leaf_8000000a=eax:0x1,ebx:0x10,ecx:0x0,edx:0x1"
        );
        let intel = Nesting::decode(1 << 5, 0x8000_0008, 0, (9, 9, 9, 9));
        assert!(intel.vmx && intel.svm.is_none());
        // No leaf 0x8000_000A: its registers are not trusted.
        assert_eq!(intel.leaf_a, (0, 0, 0, 0));
        assert!(std::format!("{intel}").starts_with("vmx=1 svm=0 npt=0 "));
    }

    #[test]
    fn nx_must_be_on_whatever_else_efer_holds() {
        use super::{EFER_NXE, nx_enabled};
        // SCE, LME and LMA as a booted long-mode CPU has them.
        let long_mode = (1 << 0) | (1 << 8) | (1 << 10);
        assert_eq!(nx_enabled(long_mode | EFER_NXE), Ok(()));
        assert_eq!(nx_enabled(EFER_NXE), Ok(()));
        assert!(nx_enabled(long_mode).is_err());
        assert!(nx_enabled(0).is_err());
        assert!(nx_enabled(!EFER_NXE).is_err());
    }

    extern crate std;

    use super::*;
    use std::vec::Vec;

    #[test]
    fn the_tsc_is_the_default_only_under_kvm_with_a_trustworthy_rate() {
        let kvm = Some(*b"KVMKVMKVM\0\0\0");
        assert_eq!(
            default_clock(kvm, true, None),
            DefaultClock::Tsc("KVM, invariant TSC")
        );
        assert!(matches!(
            default_clock(kvm, false, Some(2_400_000)),
            DefaultClock::Tsc(_)
        ));
        assert!(matches!(
            default_clock(kvm, false, None),
            DefaultClock::Hpet(_)
        ));
        for other in [
            *b"TCGTCGTCGTCG",
            *b"Microsoft Hv",
            *b"VBoxVBoxVBox",
            *b"VMwareVMware",
        ] {
            assert!(matches!(
                default_clock(Some(other), true, Some(1)),
                DefaultClock::Hpet(_)
            ));
        }
        assert_eq!(
            default_clock(None, true, None),
            DefaultClock::Hpet("no hypervisor")
        );
    }

    #[test]
    fn drift_is_parts_per_million_either_way() {
        assert_eq!(drift_ppm(2_400_000_000, 2_400_000_000), 0);
        assert_eq!(drift_ppm(2_400_000_000, 2_400_240_000), 100);
        assert_eq!(drift_ppm(2_400_000_000, 2_399_760_000), 100);
        assert_eq!(drift_ppm(2_400_000_000, 2_400_250_000), 104);
        assert_eq!(drift_ppm(0, 5), 5_000_000);
    }

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
    fn svm_needs_its_cpuid_bit() {
        assert_eq!(Svm::decode(0x8000_001F, 0, (1, 32768, u32::MAX)), None);
        assert_eq!(Svm::decode(0x8000_001F, !(1 << 2), (1, 8, 1)), None);
    }

    #[test]
    fn svm_extensions_decode_from_leaf_8000_000a() {
        // An AMD Zen 4 shape: revision 1, 32768 ASIDs, and NP, NRIPS, decode
        // assists and AVIC among EDX's other bits.
        let zen4 = Svm::decode(0x8000_0028, 1 << 2, (1, 32768, 0x1EBB_FCFF)).unwrap();
        assert_eq!(
            zen4,
            Svm {
                revision: 1,
                asids: 32768,
                npt: true,
                nrips: true,
                decode_assists: true,
                avic: true,
            }
        );
        // Each feature is its own bit.
        let only = |edx| Svm::decode(0x8000_000A, 1 << 2, (1, 16, edx)).unwrap();
        assert!(only(1).npt && !only(1).nrips);
        assert!(only(1 << 3).nrips && !only(1 << 3).npt);
        assert!(only(1 << 7).decode_assists && !only(1 << 7).avic);
        assert!(only(1 << 13).avic && !only(1 << 13).decode_assists);
        // Only EAX's low byte is the revision.
        let revision = Svm::decode(0x8000_000A, 1 << 2, (0xFF01, 0, 0))
            .unwrap()
            .revision;
        assert_eq!(revision, 1);
    }

    #[test]
    fn svm_without_its_leaf_has_no_extensions() {
        let bare = Svm::decode(0x8000_0008, 1 << 2, (1, 32768, u32::MAX)).unwrap();
        assert_eq!(bare, Svm::default());
        assert_eq!(
            std::format!("{}", bare.describe()),
            "SVM rev 0, 0 ASIDs, no extensions"
        );
    }

    #[test]
    fn svm_line_names_each_extension() {
        let edx = 1 | 1 << 3 | 1 << 7 | 1 << 13;
        let svm = Svm::decode(0x8000_000A, 1 << 2, (1, 32768, edx)).unwrap();
        assert_eq!(
            std::format!("{}", svm.describe()),
            "SVM rev 1, 32768 ASIDs, npt nrips decode-assists avic"
        );
        let npt_only = Svm::decode(0x8000_000A, 1 << 2, (1, 16, 1)).unwrap();
        assert_eq!(
            std::format!("{}", npt_only.describe()),
            "SVM rev 1, 16 ASIDs, npt"
        );
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
        // Only AMD CPUs set the SVM bit; Intel has VT-x instead.
        if cpu.svm.is_some() {
            assert_eq!(&vendor(), b"AuthenticAMD");
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
