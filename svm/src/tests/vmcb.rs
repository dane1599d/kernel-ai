//! Every named VMCB field at the offset of APM vol. 2 table B-1 (control
//! area) or B-2 (save area, from 0x400). Linux's `struct vmcb_control_area`
//! and `struct vmcb_save_area` put each at the same place.

use crate::vmcb::{CONTROL_AREA_SIZE, ControlArea, SAVE_AREA_SIZE, SaveArea, Segment, Vmcb};
use core::mem::{align_of, offset_of, size_of};

#[test]
fn the_vmcb_is_one_aligned_page_with_the_save_area_at_0x400() {
    assert_eq!(size_of::<Vmcb>(), 4096);
    assert_eq!(align_of::<Vmcb>(), 4096);
    assert_eq!(size_of::<ControlArea>(), CONTROL_AREA_SIZE);
    assert_eq!(size_of::<SaveArea>(), SAVE_AREA_SIZE);
    assert_eq!(offset_of!(Vmcb, control), 0);
    assert_eq!(offset_of!(Vmcb, save), 0x400);
    assert_eq!(size_of::<Segment>(), 16);
}

#[test]
fn the_control_area_fields_are_at_table_b1s_offsets() {
    let table: &[(usize, usize, &str)] = &[
        (
            offset_of!(ControlArea, intercepts),
            0x000,
            "intercept vectors",
        ),
        (
            offset_of!(ControlArea, pause_filter_threshold),
            0x03C,
            "PAUSE threshold",
        ),
        (
            offset_of!(ControlArea, pause_filter_count),
            0x03E,
            "PAUSE count",
        ),
        (offset_of!(ControlArea, iopm_base_pa), 0x040, "IOPM_BASE_PA"),
        (
            offset_of!(ControlArea, msrpm_base_pa),
            0x048,
            "MSRPM_BASE_PA",
        ),
        (offset_of!(ControlArea, tsc_offset), 0x050, "TSC_OFFSET"),
        (offset_of!(ControlArea, asid), 0x058, "guest ASID"),
        (offset_of!(ControlArea, tlb_control), 0x05C, "TLB_CONTROL"),
        (
            offset_of!(ControlArea, int_control),
            0x060,
            "V_TPR .. V_INTR_MASKING",
        ),
        (offset_of!(ControlArea, int_vector), 0x064, "V_INTR_VECTOR"),
        (
            offset_of!(ControlArea, int_state),
            0x068,
            "INTERRUPT_SHADOW",
        ),
        (offset_of!(ControlArea, exit_code), 0x070, "EXITCODE"),
        (offset_of!(ControlArea, exit_info_1), 0x078, "EXITINFO1"),
        (offset_of!(ControlArea, exit_info_2), 0x080, "EXITINFO2"),
        (offset_of!(ControlArea, exit_int_info), 0x088, "EXITINTINFO"),
        (
            offset_of!(ControlArea, exit_int_info_error),
            0x08C,
            "EXITINTINFO error code",
        ),
        (offset_of!(ControlArea, nested_control), 0x090, "NP_ENABLE"),
        (
            offset_of!(ControlArea, avic_apic_bar),
            0x098,
            "AVIC_APIC_BAR",
        ),
        (offset_of!(ControlArea, ghcb_pa), 0x0A0, "GHCB"),
        (offset_of!(ControlArea, event_inj), 0x0A8, "EVENTINJ"),
        (
            offset_of!(ControlArea, event_inj_error),
            0x0AC,
            "EVENTINJ error code",
        ),
        (offset_of!(ControlArea, n_cr3), 0x0B0, "N_CR3"),
        (
            offset_of!(ControlArea, virt_ext),
            0x0B8,
            "LBR_VIRTUALIZATION_ENABLE",
        ),
        (offset_of!(ControlArea, clean), 0x0C0, "VMCB clean bits"),
        (offset_of!(ControlArea, next_rip), 0x0C8, "nRIP"),
        (
            offset_of!(ControlArea, insn_len),
            0x0D0,
            "number of bytes fetched",
        ),
        (
            offset_of!(ControlArea, insn_bytes),
            0x0D1,
            "guest instruction bytes",
        ),
        (
            offset_of!(ControlArea, avic_backing_page),
            0x0E0,
            "AVIC backing page",
        ),
        (
            offset_of!(ControlArea, avic_logical_table),
            0x0F0,
            "AVIC logical table",
        ),
        (
            offset_of!(ControlArea, avic_physical_table),
            0x0F8,
            "AVIC physical table",
        ),
        (offset_of!(ControlArea, vmsa_pa), 0x108, "VMSA pointer"),
    ];
    for &(at, want, name) in table {
        assert_eq!(at, want, "{name} at {at:#x}, want {want:#x}");
    }
}

#[test]
fn the_save_area_fields_are_at_table_b2s_offsets() {
    let table: &[(usize, usize, &str)] = &[
        (offset_of!(SaveArea, es), 0x000, "ES"),
        (offset_of!(SaveArea, cs), 0x010, "CS"),
        (offset_of!(SaveArea, ss), 0x020, "SS"),
        (offset_of!(SaveArea, ds), 0x030, "DS"),
        (offset_of!(SaveArea, fs), 0x040, "FS"),
        (offset_of!(SaveArea, gs), 0x050, "GS"),
        (offset_of!(SaveArea, gdtr), 0x060, "GDTR"),
        (offset_of!(SaveArea, ldtr), 0x070, "LDTR"),
        (offset_of!(SaveArea, idtr), 0x080, "IDTR"),
        (offset_of!(SaveArea, tr), 0x090, "TR"),
        (offset_of!(SaveArea, vmpl), 0x0CA, "VMPL"),
        (offset_of!(SaveArea, cpl), 0x0CB, "CPL"),
        (offset_of!(SaveArea, efer), 0x0D0, "EFER"),
        (offset_of!(SaveArea, cr4), 0x148, "CR4"),
        (offset_of!(SaveArea, cr3), 0x150, "CR3"),
        (offset_of!(SaveArea, cr0), 0x158, "CR0"),
        (offset_of!(SaveArea, dr7), 0x160, "DR7"),
        (offset_of!(SaveArea, dr6), 0x168, "DR6"),
        (offset_of!(SaveArea, rflags), 0x170, "RFLAGS"),
        (offset_of!(SaveArea, rip), 0x178, "RIP"),
        (offset_of!(SaveArea, rsp), 0x1D8, "RSP"),
        (offset_of!(SaveArea, s_cet), 0x1E0, "S_CET"),
        (offset_of!(SaveArea, ssp), 0x1E8, "SSP"),
        (offset_of!(SaveArea, isst_addr), 0x1F0, "ISST_ADDR"),
        (offset_of!(SaveArea, rax), 0x1F8, "RAX"),
        (offset_of!(SaveArea, star), 0x200, "STAR"),
        (offset_of!(SaveArea, lstar), 0x208, "LSTAR"),
        (offset_of!(SaveArea, cstar), 0x210, "CSTAR"),
        (offset_of!(SaveArea, sfmask), 0x218, "SFMASK"),
        (offset_of!(SaveArea, kernel_gs_base), 0x220, "KernelGsBase"),
        (offset_of!(SaveArea, sysenter_cs), 0x228, "SYSENTER_CS"),
        (offset_of!(SaveArea, sysenter_esp), 0x230, "SYSENTER_ESP"),
        (offset_of!(SaveArea, sysenter_eip), 0x238, "SYSENTER_EIP"),
        (offset_of!(SaveArea, cr2), 0x240, "CR2"),
        (offset_of!(SaveArea, g_pat), 0x268, "G_PAT"),
        (offset_of!(SaveArea, dbgctl), 0x270, "DBGCTL"),
        (offset_of!(SaveArea, br_from), 0x278, "BR_FROM"),
        (offset_of!(SaveArea, br_to), 0x280, "BR_TO"),
        (offset_of!(SaveArea, last_excp_from), 0x288, "LASTEXCPFROM"),
        (offset_of!(SaveArea, last_excp_to), 0x290, "LASTEXCPTO"),
        (offset_of!(SaveArea, spec_ctrl), 0x2E0, "SPEC_CTRL"),
    ];
    for &(at, want, name) in table {
        assert_eq!(at, want, "{name} at {at:#x}, want {want:#x}");
    }
    // A segment: selector, attributes, limit, base.
    assert_eq!(offset_of!(Segment, selector), 0);
    assert_eq!(offset_of!(Segment, attrib), 2);
    assert_eq!(offset_of!(Segment, limit), 4);
    assert_eq!(offset_of!(Segment, base), 8);
}

#[test]
fn a_zeroed_vmcb_is_all_zero_bytes() {
    let vmcb = Vmcb::zeroed();
    // SAFETY: Vmcb is repr(C) with no padding (its size is the sum of its
    // fields, checked above), so its bytes are all initialized.
    let bytes = unsafe { core::slice::from_raw_parts((&vmcb as *const Vmcb).cast::<u8>(), 4096) };
    assert!(bytes.iter().all(|&b| b == 0));
}

#[test]
fn segment_attributes_pack_the_descriptors_access_byte_and_flags() {
    // A 64-bit ring 0 code segment: P, S, type 0xB; L and G.
    let code = Segment::code64(0x08, 0);
    assert_eq!(code.attrib, 0xA9B);
    // Ring 3: DPL 3 in bits 6:5.
    assert_eq!(Segment::code64(0x2B, 3).attrib, 0xAFB);
    // A flat data segment: P, S, type 3; D/B and G.
    assert_eq!(Segment::data(0x10, 0).attrib, 0xC93);
}
