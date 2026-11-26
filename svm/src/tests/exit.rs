//! Exit codes against APM vol. 2 appendix C, and the IOIO, MSR and NPF
//! exit information against 15.10.2, 15.11 and 15.25.6.

use crate::exit::{ExitCode, Ioio, Npf, msr_write};

#[test]
fn exit_codes_have_appendix_cs_values() {
    let table: &[(ExitCode, u64)] = &[
        (ExitCode::CR_READ, 0x000),
        (ExitCode::CR_WRITE, 0x010),
        (ExitCode::DR_READ, 0x020),
        (ExitCode::DR_WRITE, 0x030),
        (ExitCode::EXCEPTION, 0x040),
        (ExitCode::INTR, 0x060),
        (ExitCode::NMI, 0x061),
        (ExitCode::INIT, 0x063),
        (ExitCode::VINTR, 0x064),
        (ExitCode::RDTSC, 0x06E),
        (ExitCode::CPUID, 0x072),
        (ExitCode::IRET, 0x074),
        (ExitCode::PAUSE, 0x077),
        (ExitCode::HLT, 0x078),
        (ExitCode::IOIO, 0x07B),
        (ExitCode::MSR, 0x07C),
        (ExitCode::SHUTDOWN, 0x07F),
        (ExitCode::VMRUN, 0x080),
        (ExitCode::VMMCALL, 0x081),
        (ExitCode::RDTSCP, 0x087),
        (ExitCode::WBINVD, 0x089),
        (ExitCode::XSETBV, 0x08D),
        (ExitCode::EFER_WRITE_TRAP, 0x08F),
        (ExitCode::CR_WRITE_TRAP, 0x090),
        (ExitCode::INVPCID, 0x0A2),
        (ExitCode::NPF, 0x400),
        (ExitCode::AVIC_INCOMPLETE_IPI, 0x401),
        (ExitCode::AVIC_NOACCEL, 0x402),
        (ExitCode::VMGEXIT, 0x403),
        (ExitCode::INVALID, u64::MAX),
        (ExitCode::BUSY, u64::MAX - 1),
    ];
    for &(code, want) in table {
        assert_eq!(code.0, want, "{}", code.name());
    }
}

#[test]
fn ranged_exits_give_their_register_or_vector() {
    assert_eq!(ExitCode(0x003).cr(), Some((3, false)));
    assert_eq!(ExitCode(0x018).cr(), Some((8, true)));
    assert_eq!(ExitCode(0x027).dr(), Some((7, false)));
    assert_eq!(ExitCode(0x04E).exception(), Some(14));
    assert_eq!(ExitCode::HLT.exception(), None);
    assert_eq!(ExitCode::HLT.cr(), None);
    assert_eq!(ExitCode(0x04E).name(), "exception");
    assert_eq!(ExitCode::NPF.name(), "npf");
    assert_eq!(ExitCode(0x3FF).name(), "unknown");
}

#[test]
fn an_ioio_exit_decodes_its_port_size_and_form() {
    // OUT DX, AL to port 0x3F8 (COM1): size 8 bits (bit 4), not input.
    let out = Ioio::decode(0x03F8_0010).unwrap();
    assert_eq!(
        out,
        Ioio {
            port: 0x3F8,
            input: false,
            string: false,
            rep: false,
            size: 1,
            address_bits: 0,
            segment: 0,
        }
    );
    // REP INSW at port 0x1F0 with a 64-bit address: IN, STR, REP, SZ16, A64.
    let ins = Ioio::decode(0x01F0_0000 | 1 | 1 << 2 | 1 << 3 | 1 << 5 | 1 << 9).unwrap();
    assert!(ins.input && ins.string && ins.rep);
    assert_eq!((ins.port, ins.size, ins.address_bits), (0x1F0, 2, 64));
    // What the CPU wrote reads back the same.
    for info in [0x03F8_0010, 0x01F0_022D, 0x0CF8_0040 | 1] {
        assert_eq!(Ioio::decode(info).unwrap().encode(), info);
    }
    // No size bit, or two: not one the CPU writes.
    assert_eq!(Ioio::decode(0x0080_0000), None);
    assert_eq!(Ioio::decode(0x0080_0030), None);
}

#[test]
fn an_msr_exit_says_read_or_write_and_an_npf_its_cause() {
    assert!(!msr_write(0));
    assert!(msr_write(1));
    // A write to a present page with the guest's walk done: P, RW, final.
    let npf = Npf::decode(1 | 1 << 1 | 1 << 32, 0xFEE0_0000);
    assert!(npf.present && npf.write && npf.final_address);
    assert!(!npf.fetch && !npf.page_table && !npf.user && !npf.reserved);
    assert_eq!(npf.gpa, 0xFEE0_0000);
    // A missing page met while walking the guest's own page tables.
    let walk = Npf::decode(1 << 33, 0x1000);
    assert!(!walk.present && walk.page_table && !walk.final_address);
}
