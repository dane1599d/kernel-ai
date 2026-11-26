//! Exit reasons against vol. 3D appendix C, and the qualifications against
//! vol. 3C tables 28-1 to 28-7.

use crate::exit::{
    ALL, BadExit, BasicExit, CrAccess, CrKind, DrAccess, EntryFailure, EptViolation, ExitReason,
    IoAccess,
};

#[test]
fn basic_exits_have_appendix_cs_numbers() {
    let table: &[(BasicExit, u16)] = &[
        (BasicExit::ExceptionOrNmi, 0),
        (BasicExit::ExternalInterrupt, 1),
        (BasicExit::TripleFault, 2),
        (BasicExit::InitSignal, 3),
        (BasicExit::StartupIpi, 4),
        (BasicExit::IoSmi, 5),
        (BasicExit::OtherSmi, 6),
        (BasicExit::InterruptWindow, 7),
        (BasicExit::NmiWindow, 8),
        (BasicExit::TaskSwitch, 9),
        (BasicExit::Cpuid, 10),
        (BasicExit::Getsec, 11),
        (BasicExit::Hlt, 12),
        (BasicExit::Invd, 13),
        (BasicExit::Rsm, 17),
        (BasicExit::Vmcall, 18),
        (BasicExit::Vmxoff, 26),
        (BasicExit::Vmxon, 27),
        (BasicExit::CrAccess, 28),
        (BasicExit::DrAccess, 29),
        (BasicExit::Io, 30),
        (BasicExit::Rdmsr, 31),
        (BasicExit::Wrmsr, 32),
        (BasicExit::EntryInvalidGuestState, 33),
        (BasicExit::EntryMsrLoading, 34),
        (BasicExit::Mwait, 36),
        (BasicExit::MonitorTrapFlag, 37),
        (BasicExit::Monitor, 39),
        (BasicExit::Pause, 40),
        (BasicExit::EntryMachineCheck, 41),
        (BasicExit::TprBelowThreshold, 43),
        (BasicExit::ApicAccess, 44),
        (BasicExit::EptViolation, 48),
        (BasicExit::EptMisconfiguration, 49),
        (BasicExit::Invept, 50),
        (BasicExit::PreemptionTimer, 52),
        (BasicExit::Invvpid, 53),
        (BasicExit::Xsetbv, 55),
        (BasicExit::Vmfunc, 59),
        (BasicExit::PmlFull, 62),
        (BasicExit::Xrstors, 64),
        (BasicExit::Pconfig, 65),
        (BasicExit::Tpause, 68),
        (BasicExit::Enclv, 70),
        (BasicExit::EnqcmdPasidFailure, 72),
        (BasicExit::BusLock, 74),
        (BasicExit::InstructionTimeout, 75),
        (BasicExit::Tdcall, 77),
        (BasicExit::RdmsrImmediate, 84),
        (BasicExit::WrmsrImmediate, 85),
    ];
    for &(exit, raw) in table {
        assert_eq!(exit as u16, raw, "{exit:?}");
        assert_eq!(BasicExit::from_raw(raw), Ok(exit));
    }
    // ALL is in order, each once, and from_raw finds each.
    for pair in ALL.windows(2) {
        assert!((pair[0] as u16) < (pair[1] as u16));
    }
    for &exit in ALL {
        assert_eq!(BasicExit::from_raw(exit as u16), Ok(exit));
    }
}

#[test]
fn unused_and_future_numbers_are_unknown() {
    for raw in [35, 38, 42, 71, 78, 79, 80, 83, 86, 0xFFFF] {
        assert_eq!(BasicExit::from_raw(raw), Err(BadExit::Unknown(raw)));
    }
}

#[test]
fn the_exit_reason_fields_flags() {
    let hlt = ExitReason::decode(12).unwrap();
    assert_eq!(hlt.basic, BasicExit::Hlt);
    assert!(!hlt.entry_failed && !hlt.enclave && !hlt.pending_mtf && !hlt.from_root);
    let failed = ExitReason::decode(0x8000_0021).unwrap();
    assert_eq!(failed.basic, BasicExit::EntryInvalidGuestState);
    assert!(failed.entry_failed);
    assert_eq!(failed.encode(), 0x8000_0021);
    let flagged = ExitReason::decode(48 | 1 << 27 | 1 << 28 | 1 << 29).unwrap();
    assert!(flagged.enclave && flagged.pending_mtf && flagged.from_root);
    assert_eq!(flagged.encode(), 48 | 1 << 27 | 1 << 28 | 1 << 29);
}

#[test]
fn a_bad_exit_reason_field_is_refused() {
    // Reserved bits 26:16 and 30.
    assert_eq!(
        ExitReason::decode(12 | 1 << 16),
        Err(BadExit::Reserved(12 | 1 << 16))
    );
    assert_eq!(
        ExitReason::decode(12 | 1 << 30),
        Err(BadExit::Reserved(12 | 1 << 30))
    );
    // A failed entry that names an ordinary exit, and an entry-failure
    // reason without bit 31.
    assert_eq!(
        ExitReason::decode(0x8000_000C),
        Err(BadExit::FailureMismatch(0x8000_000C))
    );
    assert_eq!(ExitReason::decode(33), Err(BadExit::FailureMismatch(33)));
    assert_eq!(ExitReason::decode(41), Err(BadExit::FailureMismatch(41)));
    assert_eq!(ExitReason::decode(35), Err(BadExit::Unknown(35)));
}

#[test]
fn a_cr_access_decodes_its_register_and_kind() {
    // MOV CR3, RAX: CR 3, type 0, register 0.
    assert_eq!(
        CrAccess::decode(0x003),
        Ok(CrAccess {
            cr: 3,
            kind: CrKind::MovTo { register: 0 }
        })
    );
    // MOV R10, CR8: type 1, register 10.
    assert_eq!(
        CrAccess::decode(0x0A18),
        Ok(CrAccess {
            cr: 8,
            kind: CrKind::MovFrom { register: 10 }
        })
    );
    assert_eq!(
        CrAccess::decode(0x20),
        Ok(CrAccess {
            cr: 0,
            kind: CrKind::Clts
        })
    );
    // LMSW of 0x0011 from memory.
    assert_eq!(
        CrAccess::decode(0x0011_0070),
        Ok(CrAccess {
            cr: 0,
            kind: CrKind::Lmsw {
                source: 0x11,
                memory: true
            }
        })
    );
    // CR2 never exits; MOV from CR0 never exits; CLTS is CR0's; bit 7 and
    // the high half are reserved.
    for q in [0x002, 0x010, 0x023, 0x080, 1 << 32] {
        assert_eq!(
            CrAccess::decode(q),
            Err(BadExit::Qualification(q)),
            "{q:#x}"
        );
    }
}

#[test]
fn a_dr_access_and_an_io_access() {
    assert_eq!(
        DrAccess::decode(0x0317),
        Ok(DrAccess {
            dr: 7,
            read: true,
            register: 3
        })
    );
    assert_eq!(DrAccess::decode(0x0008), Err(BadExit::Qualification(0x8)));
    // `in al, 0x60`: size 1, IN, an immediate port.
    let io = IoAccess::decode(0x0060_0048).unwrap();
    assert_eq!(
        io,
        IoAccess {
            port: 0x60,
            size: 1,
            input: true,
            string: false,
            rep: false,
            immediate: true
        }
    );
    assert_eq!(io.encode(), 0x0060_0048);
    // `rep outsd` to DX = 0x3F8.
    let outs = IoAccess::decode(0x03F8_0033).unwrap();
    assert_eq!(
        (outs.size, outs.input, outs.string, outs.rep),
        (4, false, true, true)
    );
    assert_eq!(IoAccess::decode(outs.encode()), Ok(outs));
    // Size 3 (field value 2) is not a size; bit 7 is reserved.
    assert_eq!(IoAccess::decode(0x2), Err(BadExit::Qualification(0x2)));
    assert_eq!(IoAccess::decode(0x80), Err(BadExit::Qualification(0x80)));
}

#[test]
fn an_ept_violation_says_the_access_and_the_rights() {
    // A write to a read-only page, through the final translation.
    let v = EptViolation::decode(0x0000_0000_0000_018A, 0xFEE0_0000);
    assert_eq!(v.gpa, 0xFEE0_0000);
    assert!(v.write && !v.read && !v.fetch);
    assert!(v.readable && !v.writable && !v.executable);
    assert!(v.linear_valid && v.final_address && !v.not_present());
    // A fetch from an unmapped page, with NMI unblocking.
    let u = EptViolation::decode(1 << 2 | 1 << 12, 0x1000);
    assert!(u.fetch && u.not_present() && u.nmi_unblocking);
    // Unnamed higher bits only add detail.
    assert!(EptViolation::decode(0x18A | 1 << 40, 0).write);
}

#[test]
fn entry_failure_qualifications() {
    assert_eq!(EntryFailure::decode(0), Ok(EntryFailure::Default));
    assert_eq!(EntryFailure::decode(2), Ok(EntryFailure::Pdpte));
    assert_eq!(EntryFailure::decode(3), Ok(EntryFailure::NmiInjection));
    assert_eq!(EntryFailure::decode(4), Ok(EntryFailure::VmcsLinkPointer));
    assert_eq!(EntryFailure::decode(1), Err(BadExit::Qualification(1)));
    assert_eq!(
        EntryFailure::decode(u64::MAX),
        Err(BadExit::Qualification(u64::MAX))
    );
}
