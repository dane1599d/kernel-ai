//! The VMCB (APM vol. 2, appendix B): one 4 KiB page, 4 KiB aligned, holding
//! the control area at offset 0 (1 KiB) and the state save area at 0x400.
//! Each field below sits at the offset its doc comment gives, the APM's
//! table B-1 offset for the control area and table B-2's (from 0x400) for the
//! save area; `tests::vmcb` checks every one with `offset_of!`.
//!
//! Only the fields a non-SEV guest uses are named. Ranges the APM reserves,
//! or gives to SEV-ES, AVIC tables the kernel does not use yet, PML or bus
//! lock counting, are `reserved_*` bytes the kernel leaves zero: VMRUN checks
//! some reserved fields are zero (APM 15.5.1, "Canonicalization and
//! Consistency Checks").

/// The control area: what is intercepted and how the guest runs (table B-1).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlArea {
    /// 0x000-0x017: the six intercept vectors, 32 bits each: CR reads and
    /// writes, DR reads and writes, exceptions, then three of instructions
    /// and events ([`crate::Intercepts`]).
    pub intercepts: [u32; 6],
    /// 0x018-0x03B.
    pub reserved_018: [u32; 9],
    /// 0x03C: PAUSE filter threshold.
    pub pause_filter_threshold: u16,
    /// 0x03E: PAUSE filter count.
    pub pause_filter_count: u16,
    /// 0x040: physical address of the 12 KiB I/O permission map.
    pub iopm_base_pa: u64,
    /// 0x048: physical address of the 8 KiB MSR permission map.
    pub msrpm_base_pa: u64,
    /// 0x050: added to the host's TSC for the guest's RDTSC.
    pub tsc_offset: u64,
    /// 0x058: the guest's ASID; 0 is the host's and is refused by VMRUN.
    pub asid: u32,
    /// 0x05C: TLB_CONTROL ([`TLB_FLUSH_NONE`] and the rest).
    pub tlb_control: u8,
    /// 0x05D-0x05F.
    pub reserved_05d: [u8; 3],
    /// 0x060: V_TPR, V_IRQ, V_INTR_PRIO, V_IGN_TPR, V_INTR_MASKING and the
    /// rest ([`V_IRQ`], [`V_INTR_MASKING`]).
    pub int_control: u32,
    /// 0x064: V_INTR_VECTOR, the virtual interrupt's vector (bits 7:0).
    pub int_vector: u32,
    /// 0x068: bit 0 the guest is in an interrupt shadow; bit 1 GUEST_INTERRUPT_MASK.
    pub int_state: u64,
    /// 0x070: EXITCODE ([`crate::ExitCode`]).
    pub exit_code: u64,
    /// 0x078: EXITINFO1.
    pub exit_info_1: u64,
    /// 0x080: EXITINFO2.
    pub exit_info_2: u64,
    /// 0x088: EXITINTINFO, an event the exit interrupted ([`crate::Event`]).
    pub exit_int_info: u32,
    /// 0x08C: its error code.
    pub exit_int_info_error: u32,
    /// 0x090: bit 0 NP_ENABLE ([`NP_ENABLE`]); bits 1 and 2 SEV and SEV-ES.
    pub nested_control: u64,
    /// 0x098: AVIC APIC_BAR.
    pub avic_apic_bar: u64,
    /// 0x0A0: guest physical address of the GHCB (SEV-ES).
    pub ghcb_pa: u64,
    /// 0x0A8: EVENTINJ, an event to inject at VMRUN ([`crate::Event`]).
    pub event_inj: u32,
    /// 0x0AC: its error code.
    pub event_inj_error: u32,
    /// 0x0B0: N_CR3, the nested page table's root ([`crate::npt::ncr3`]).
    pub n_cr3: u64,
    /// 0x0B8: bit 0 LBR virtualization, bit 1 virtualized VMSAVE/VMLOAD.
    pub virt_ext: u64,
    /// 0x0C0: VMCB clean bits ([`Clean`]).
    pub clean: u32,
    /// 0x0C4.
    pub reserved_0c4: u32,
    /// 0x0C8: nRIP, the next instruction's RIP on an intercepted
    /// instruction, with NRIPS (CPUID 0x8000_000A EDX bit 3).
    pub next_rip: u64,
    /// 0x0D0: bytes fetched for a #NPF or #PF intercept, with DecodeAssists.
    pub insn_len: u8,
    /// 0x0D1-0x0DF: those bytes.
    pub insn_bytes: [u8; 15],
    /// 0x0E0: AVIC backing page.
    pub avic_backing_page: u64,
    /// 0x0E8-0x0EF.
    pub reserved_0e8: u64,
    /// 0x0F0: AVIC logical table.
    pub avic_logical_table: u64,
    /// 0x0F8: AVIC physical table.
    pub avic_physical_table: u64,
    /// 0x100-0x107.
    pub reserved_100: u64,
    /// 0x108: VMSA pointer (SEV-ES).
    pub vmsa_pa: u64,
    /// 0x110-0x3FF: SEV, bus lock, PML and the hypervisor's own 32 bytes at
    /// 0x3E0; none of them is used.
    pub reserved_110: [u8; 0x2F0],
}

/// A segment register as the save area holds it (table B-2): 16 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Segment {
    pub selector: u16,
    /// The descriptor's attributes packed in 12 bits ([`Segment::attrib`]).
    pub attrib: u16,
    pub limit: u32,
    pub base: u64,
}

impl Segment {
    /// The VMCB's attribute word from a GDT descriptor's access byte (bits
    /// 47:40) and flags nibble (bits 55:52): type, S, DPL and P in bits 7:0,
    /// then AVL, L, D/B and G in bits 11:8.
    pub const fn attrib(access: u8, flags: u8) -> u16 {
        access as u16 | ((flags as u16 & 0xF) << 8)
    }

    /// A flat 64-bit code segment at ring `dpl` (L set, D/B clear).
    pub const fn code64(selector: u16, dpl: u8) -> Self {
        Self {
            selector,
            attrib: Self::attrib(0x9B | ((dpl & 3) << 5), 0b1010),
            limit: 0xFFFF_FFFF,
            base: 0,
        }
    }

    /// A flat writable data segment at ring `dpl`.
    pub const fn data(selector: u16, dpl: u8) -> Self {
        Self {
            selector,
            attrib: Self::attrib(0x93 | ((dpl & 3) << 5), 0b1100),
            limit: 0xFFFF_FFFF,
            base: 0,
        }
    }
}

/// The state save area of a guest without SEV-ES (table B-2), at VMCB offset
/// 0x400; the offsets below are from 0x400.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SaveArea {
    /// 0x000.
    pub es: Segment,
    /// 0x010.
    pub cs: Segment,
    /// 0x020.
    pub ss: Segment,
    /// 0x030.
    pub ds: Segment,
    /// 0x040.
    pub fs: Segment,
    /// 0x050.
    pub gs: Segment,
    /// 0x060: only limit and base are used.
    pub gdtr: Segment,
    /// 0x070.
    pub ldtr: Segment,
    /// 0x080: only limit and base are used.
    pub idtr: Segment,
    /// 0x090.
    pub tr: Segment,
    /// 0x0A0-0x0C9.
    pub reserved_0a0: [u8; 42],
    /// 0x0CA: VMPL (SEV-SNP).
    pub vmpl: u8,
    /// 0x0CB: the guest's CPL; VMRUN takes it from here, not from SS.
    pub cpl: u8,
    /// 0x0CC-0x0CF.
    pub reserved_0cc: u32,
    /// 0x0D0: the guest's EFER; SVME must be set or VMRUN fails.
    pub efer: u64,
    /// 0x0D8-0x147.
    pub reserved_0d8: [u8; 112],
    /// 0x148.
    pub cr4: u64,
    /// 0x150.
    pub cr3: u64,
    /// 0x158.
    pub cr0: u64,
    /// 0x160.
    pub dr7: u64,
    /// 0x168.
    pub dr6: u64,
    /// 0x170.
    pub rflags: u64,
    /// 0x178.
    pub rip: u64,
    /// 0x180-0x1D7.
    pub reserved_180: [u8; 88],
    /// 0x1D8.
    pub rsp: u64,
    /// 0x1E0: S_CET.
    pub s_cet: u64,
    /// 0x1E8: SSP.
    pub ssp: u64,
    /// 0x1F0: ISST_ADDR.
    pub isst_addr: u64,
    /// 0x1F8: RAX; the other general registers are the hypervisor's to save.
    pub rax: u64,
    /// 0x200.
    pub star: u64,
    /// 0x208.
    pub lstar: u64,
    /// 0x210.
    pub cstar: u64,
    /// 0x218.
    pub sfmask: u64,
    /// 0x220.
    pub kernel_gs_base: u64,
    /// 0x228.
    pub sysenter_cs: u64,
    /// 0x230.
    pub sysenter_esp: u64,
    /// 0x238.
    pub sysenter_eip: u64,
    /// 0x240.
    pub cr2: u64,
    /// 0x248-0x267.
    pub reserved_248: [u8; 32],
    /// 0x268: the guest's PAT, used with nested paging.
    pub g_pat: u64,
    /// 0x270: DBGCTL.
    pub dbgctl: u64,
    /// 0x278.
    pub br_from: u64,
    /// 0x280.
    pub br_to: u64,
    /// 0x288.
    pub last_excp_from: u64,
    /// 0x290.
    pub last_excp_to: u64,
    /// 0x298-0x2DF.
    pub reserved_298: [u8; 72],
    /// 0x2E0: the guest's SPEC_CTRL.
    pub spec_ctrl: u64,
}

/// The save area's size without SEV-ES: 744 bytes (0x2E8).
pub const SAVE_AREA_SIZE: usize = 0x2E8;
/// The control area's size, and the save area's offset in the VMCB.
pub const CONTROL_AREA_SIZE: usize = 0x400;

/// A VMCB: a 4 KiB page, 4 KiB aligned, as VMRUN's RAX names it.
#[repr(C, align(4096))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vmcb {
    pub control: ControlArea,
    pub save: SaveArea,
    /// The page's rest after the save area: zero.
    pub reserved: [u8; 4096 - CONTROL_AREA_SIZE - SAVE_AREA_SIZE],
}

impl Vmcb {
    /// An all-zero VMCB: every intercept off, ASID 0 (which VMRUN refuses),
    /// so the caller sets at least the ASID and the VMRUN intercept
    /// ([`crate::Intercepts::required`]).
    pub const fn zeroed() -> Self {
        Self {
            control: ControlArea::zeroed(),
            save: SaveArea::zeroed(),
            reserved: [0; 4096 - CONTROL_AREA_SIZE - SAVE_AREA_SIZE],
        }
    }
}

impl ControlArea {
    pub const fn zeroed() -> Self {
        Self {
            intercepts: [0; 6],
            reserved_018: [0; 9],
            pause_filter_threshold: 0,
            pause_filter_count: 0,
            iopm_base_pa: 0,
            msrpm_base_pa: 0,
            tsc_offset: 0,
            asid: 0,
            tlb_control: 0,
            reserved_05d: [0; 3],
            int_control: 0,
            int_vector: 0,
            int_state: 0,
            exit_code: 0,
            exit_info_1: 0,
            exit_info_2: 0,
            exit_int_info: 0,
            exit_int_info_error: 0,
            nested_control: 0,
            avic_apic_bar: 0,
            ghcb_pa: 0,
            event_inj: 0,
            event_inj_error: 0,
            n_cr3: 0,
            virt_ext: 0,
            clean: 0,
            reserved_0c4: 0,
            next_rip: 0,
            insn_len: 0,
            insn_bytes: [0; 15],
            avic_backing_page: 0,
            reserved_0e8: 0,
            avic_logical_table: 0,
            avic_physical_table: 0,
            reserved_100: 0,
            vmsa_pa: 0,
            reserved_110: [0; 0x2F0],
        }
    }
}

impl SaveArea {
    pub const fn zeroed() -> Self {
        const NONE: Segment = Segment {
            selector: 0,
            attrib: 0,
            limit: 0,
            base: 0,
        };
        Self {
            es: NONE,
            cs: NONE,
            ss: NONE,
            ds: NONE,
            fs: NONE,
            gs: NONE,
            gdtr: NONE,
            ldtr: NONE,
            idtr: NONE,
            tr: NONE,
            reserved_0a0: [0; 42],
            vmpl: 0,
            cpl: 0,
            reserved_0cc: 0,
            efer: 0,
            reserved_0d8: [0; 112],
            cr4: 0,
            cr3: 0,
            cr0: 0,
            dr7: 0,
            dr6: 0,
            rflags: 0,
            rip: 0,
            reserved_180: [0; 88],
            rsp: 0,
            s_cet: 0,
            ssp: 0,
            isst_addr: 0,
            rax: 0,
            star: 0,
            lstar: 0,
            cstar: 0,
            sfmask: 0,
            kernel_gs_base: 0,
            sysenter_cs: 0,
            sysenter_esp: 0,
            sysenter_eip: 0,
            cr2: 0,
            reserved_248: [0; 32],
            g_pat: 0,
            dbgctl: 0,
            br_from: 0,
            br_to: 0,
            last_excp_from: 0,
            last_excp_to: 0,
            reserved_298: [0; 72],
            spec_ctrl: 0,
        }
    }
}

// The layout the CPU reads, checked when the crate builds, for every target:
// the sizes and the fields VMRUN and #VMEXIT use most (APM tables B-1, B-2).
const _: () = {
    use core::mem::{align_of, offset_of, size_of};
    assert!(size_of::<Vmcb>() == 4096 && align_of::<Vmcb>() == 4096);
    assert!(size_of::<ControlArea>() == CONTROL_AREA_SIZE);
    assert!(size_of::<SaveArea>() == SAVE_AREA_SIZE);
    assert!(size_of::<Segment>() == 16);
    assert!(offset_of!(Vmcb, save) == 0x400);
    assert!(offset_of!(ControlArea, iopm_base_pa) == 0x040);
    assert!(offset_of!(ControlArea, msrpm_base_pa) == 0x048);
    assert!(offset_of!(ControlArea, asid) == 0x058);
    assert!(offset_of!(ControlArea, exit_code) == 0x070);
    assert!(offset_of!(ControlArea, exit_info_1) == 0x078);
    assert!(offset_of!(ControlArea, exit_info_2) == 0x080);
    assert!(offset_of!(ControlArea, exit_int_info) == 0x088);
    assert!(offset_of!(ControlArea, nested_control) == 0x090);
    assert!(offset_of!(ControlArea, event_inj) == 0x0A8);
    assert!(offset_of!(ControlArea, n_cr3) == 0x0B0);
    assert!(offset_of!(ControlArea, clean) == 0x0C0);
    assert!(offset_of!(ControlArea, next_rip) == 0x0C8);
    assert!(offset_of!(SaveArea, cpl) == 0x0CB);
    assert!(offset_of!(SaveArea, efer) == 0x0D0);
    assert!(offset_of!(SaveArea, cr4) == 0x148);
    assert!(offset_of!(SaveArea, rip) == 0x178);
    assert!(offset_of!(SaveArea, rsp) == 0x1D8);
    assert!(offset_of!(SaveArea, rax) == 0x1F8);
    assert!(offset_of!(SaveArea, cr2) == 0x240);
    assert!(offset_of!(SaveArea, g_pat) == 0x268);
    assert!(offset_of!(SaveArea, spec_ctrl) == 0x2E0);
};

/// TLB_CONTROL: flush nothing.
pub const TLB_FLUSH_NONE: u8 = 0;
/// TLB_CONTROL: flush every ASID's entries (the host's too).
pub const TLB_FLUSH_ALL: u8 = 1;
/// TLB_CONTROL: flush this guest's ASID, with FlushByAsid (CPUID 0x8000_000A
/// EDX bit 6).
pub const TLB_FLUSH_ASID: u8 = 3;
/// TLB_CONTROL: flush this ASID's non-global entries.
pub const TLB_FLUSH_ASID_LOCAL: u8 = 7;

/// int_control: V_TPR, the virtual TPR (bits 3:0).
pub const V_TPR: u32 = 0xF;
/// int_control: a virtual interrupt is pending.
pub const V_IRQ: u32 = 1 << 8;
/// int_control: V_INTR_PRIO (bits 19:16).
pub const V_INTR_PRIO_SHIFT: u32 = 16;
/// int_control: the virtual interrupt ignores the TPR.
pub const V_IGN_TPR: u32 = 1 << 20;
/// int_control: the guest's EFLAGS.IF and TPR mask only virtual interrupts;
/// the host's physical ones still reach the host.
pub const V_INTR_MASKING: u32 = 1 << 24;

/// nested_control: nested paging on; N_CR3 and G_PAT are used.
pub const NP_ENABLE: u64 = 1 << 0;

/// VMCB clean bits (APM 15.15.3): a set bit tells the CPU that field group
/// has not changed since the last VMRUN of this VMCB on this CPU, so it may
/// use a cached copy. A VMCB new to a CPU, or moved to another one, must run
/// with `clean` 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Clean {
    /// Intercept vectors, TSC offset, PAUSE filter.
    Intercepts = 1 << 0,
    /// IOPM and MSRPM base addresses.
    Iopm = 1 << 1,
    /// ASID.
    Asid = 1 << 2,
    /// V_TPR, V_IRQ, V_INTR_PRIO, V_IGN_TPR, V_INTR_MASKING, V_INTR_VECTOR.
    Tpr = 1 << 3,
    /// NP_ENABLE, N_CR3, G_PAT.
    Np = 1 << 4,
    /// CR0, CR3, CR4, EFER.
    Cr = 1 << 5,
    /// DR6, DR7.
    Dr = 1 << 6,
    /// GDTR, IDTR.
    Dt = 1 << 7,
    /// CS, DS, SS, ES, CPL.
    Seg = 1 << 8,
    /// CR2.
    Cr2 = 1 << 9,
    /// DBGCTL and the LBR fields.
    Lbr = 1 << 10,
    /// The AVIC fields.
    Avic = 1 << 11,
}
