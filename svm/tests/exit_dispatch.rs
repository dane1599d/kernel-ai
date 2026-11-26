//! `svm::exit::dispatch` over every exit code `svm` names, against APM
//! vol. 2 rev. 3.38: 15.7.1 (nRIP), 15.10.2 (IOIO), 15.12 (exceptions),
//! 15.14 and 15.25.6 (nested faults).

mod common;

use common::{GIB, MIB, guest_ram};
use svm::Intercepts;
use svm::exit::{
    APIC_WINDOW, Action, Exit, ExitCode, Fault, Guest, Hand, IOAPIC, Insn, Ioio, Kernel, Kill,
    LAPIC, Next, Npf, dispatch, intercept, intercepts, length,
};
use svm::npt::{Access, Layout, Region};
use svm::{Event, Intercept};

/// Every code `svm` names: the single ones, and each of the ranges.
pub fn named() -> Vec<ExitCode> {
    let mut codes: Vec<ExitCode> = (0..=0x403).map(ExitCode).collect();
    codes.extend([ExitCode::INVALID, ExitCode::BUSY]);
    codes.retain(|c| c.name() != "unknown");
    codes
}

fn exit(code: ExitCode, info1: u64, info2: u64, nrip: Option<u64>) -> Exit {
    Exit {
        code,
        info1,
        info2,
        nrip,
    }
}

fn run(code: ExitCode, info1: u64, info2: u64, nrip: Option<u64>) -> Action {
    let ram = guest_ram();
    let guest = Guest {
        intercepts: intercepts().with(Intercept::VINTR),
        ram: &ram,
    };
    dispatch(&exit(code, info1, info2, nrip), &guest)
}

/// As [`run`], with every intercept set, for exits the kernel's set leaves out.
fn run_all(code: ExitCode, info1: u64, info2: u64, nrip: Option<u64>) -> Action {
    let ram = guest_ram();
    let guest = Guest {
        intercepts: Intercepts([u32::MAX; 6]),
        ram: &ram,
    };
    dispatch(&exit(code, info1, info2, nrip), &guest)
}

fn inject(vector: u8, error: Option<u32>, cr2: Option<u64>) -> Action {
    Action::Inject(Fault {
        event: Event::exception(vector, error).unwrap(),
        cr2,
    })
}

#[test]
fn every_named_code_maps_to_an_action_and_no_other_does() {
    let ram = guest_ram();
    let all = Guest {
        intercepts: Intercepts([u32::MAX; 6]),
        ram: &ram,
    };
    let named = named();
    // 16 CR reads, 16 writes, DRs alike, 32 exceptions, 16 CR write traps,
    // and the single exits.
    assert!(named.len() > 16 * 5 + 32 + 40, "{}", named.len());
    for &code in &named {
        for nrip in [Some(0x1000), None] {
            let action = dispatch(&exit(code, 0, 0, nrip), &all);
            assert!(
                !matches!(
                    action,
                    Action::Kill(Kill::Unknown(_) | Kill::Unrequested(_))
                ),
                "{}: {action:?}",
                code.name()
            );
        }
    }
    for code in [0x0A7, 0x0FF, 0x3FF, 0x404, 0x1_0000, u64::MAX - 2] {
        let action = dispatch(&exit(ExitCode(code), 0, 0, None), &all);
        assert_eq!(action, Action::Kill(Kill::Unknown(ExitCode(code))));
    }
}

#[test]
fn what_the_kernel_asks_for_it_handles() {
    // Each exit the kernel's intercept set asks for has a handler: none of
    // them kills the guest as unsupported, unrequested or unknown.
    for code in named() {
        let Some(bit) = intercept(code) else { continue };
        if !intercepts().contains(bit) {
            continue;
        }
        let action = run(code, 0, 0, Some(0x2000));
        let unhandled = matches!(
            action,
            Action::Kill(Kill::Unsupported(_) | Kill::Unrequested(_) | Kill::Unknown(_))
        );
        assert!(!unhandled, "{}: {action:?}", code.name());
    }
    // And it asks for at least what VMRUN and the host's safety need.
    let required = Intercepts::required();
    for (word, bits) in required.0.iter().enumerate() {
        assert_eq!(intercepts().0[word] & bits, *bits);
    }
}

#[test]
fn an_exit_the_vmcb_did_not_ask_for_kills_the_guest() {
    // The kernel never intercepts CR3 writes (NPT leaves CR3 to the guest).
    assert_eq!(
        run(ExitCode(0x013), 0, 0, Some(0x10)),
        Action::Kill(Kill::Unrequested(ExitCode(0x013)))
    );
    // Nor VINTR unless it waits for a window.
    let ram = guest_ram();
    let guest = Guest {
        intercepts: intercepts(),
        ram: &ram,
    };
    let vintr = exit(ExitCode::VINTR, 0, 0, None);
    assert_eq!(
        dispatch(&vintr, &guest),
        Action::Kill(Kill::Unrequested(ExitCode::VINTR))
    );
    assert_eq!(
        run(ExitCode::VINTR, 0, 0, None),
        Action::Kernel(Kernel::InterruptWindow, Next::Stay)
    );
}

#[test]
fn intercept_bits_follow_exit_codes() {
    let pairs = [
        (ExitCode(0x003), Intercept::cr_read(3)),
        (ExitCode(0x018), Intercept::cr_write(8)),
        (ExitCode(0x027), Intercept::dr_read(7)),
        (ExitCode(0x031), Intercept::dr_write(1)),
        (ExitCode(0x04E), Intercept::exception(14)),
        (ExitCode::INTR, Intercept::INTR),
        (ExitCode::RDTSC, Intercept::RDTSC),
        (ExitCode::CPUID, Intercept::CPUID),
        (ExitCode::IOIO, Intercept::IOIO_PROT),
        (ExitCode::MSR, Intercept::MSR_PROT),
        (ExitCode::SHUTDOWN, Intercept::SHUTDOWN),
        (ExitCode::VMRUN, Intercept::VMRUN),
        (ExitCode::RDPRU, Intercept::RDPRU),
        (ExitCode::EFER_WRITE_TRAP, Intercept::EFER_WRITE_TRAP),
        (ExitCode(0x093), Intercept::cr_write_trap(3)),
        (ExitCode::INVPCID, Intercept::INVPCID),
        (ExitCode::BUS_LOCK, Intercept::BUS_LOCK),
        (ExitCode::IDLE_HLT, Intercept::IDLE_HLT),
    ];
    for (code, bit) in pairs {
        assert_eq!(intercept(code), Some(bit), "{}", code.name());
    }
    for code in [
        ExitCode::NPF,
        ExitCode::VMGEXIT,
        ExitCode::INVALID,
        ExitCode::BUSY,
    ] {
        assert_eq!(intercept(code), None);
    }
}

#[test]
fn emulated_instructions_step_to_nrip_or_decode_without_it() {
    let cases = [
        (ExitCode::CPUID, 0, Kernel::Cpuid, Insn::Cpuid),
        (ExitCode::HLT, 0, Kernel::Hlt, Insn::Hlt),
        (ExitCode::XSETBV, 0, Kernel::Xsetbv, Insn::Xsetbv),
        (ExitCode::INVD, 0, Kernel::Flush, Insn::Invd),
        (ExitCode::WBINVD, 0, Kernel::Flush, Insn::Wbinvd),
        (ExitCode::MSR, 0, Kernel::Msr { write: false }, Insn::Rdmsr),
        (ExitCode::MSR, 1, Kernel::Msr { write: true }, Insn::Wrmsr),
    ];
    for (code, info1, what, insn) in cases {
        assert_eq!(
            run(code, info1, 0, Some(0xFFFF_8000_0000_1234)),
            Action::Kernel(what, Next::To(0xFFFF_8000_0000_1234))
        );
        // TCG: no nRIP, so the kernel decodes the instruction at RIP.
        assert_eq!(
            run(code, info1, 0, None),
            Action::Kernel(what, Next::Decode(insn))
        );
    }
    // An MSR exit is RDMSR or WRMSR, nothing else (APM 15.11).
    assert_eq!(
        run(ExitCode::MSR, 2, 0, None),
        Action::Kill(Kill::BadInfo(ExitCode::MSR))
    );
    for code in [ExitCode::INTR, ExitCode::NMI, ExitCode::SMI, ExitCode::INIT] {
        assert_eq!(
            run(code, 0, 0, None),
            Action::Kernel(Kernel::HostEvent, Next::Stay)
        );
    }
    assert_eq!(
        run(ExitCode::PAUSE, 0, 0, Some(9)),
        Action::Kernel(Kernel::Pause, Next::Stay)
    );
}

#[test]
fn port_io_goes_to_vmm_and_steps_by_exitinfo2_even_without_nrips() {
    // OUT DX, AL to COM1; EXITINFO2 is the next RIP (APM 15.10.2).
    let out = Ioio::decode(0x03F8_0010).unwrap();
    for nrip in [Some(0x7777), None] {
        assert_eq!(
            run(ExitCode::IOIO, 0x03F8_0010, 0x1235, nrip),
            Action::Vmm(Hand::Pio(out), Next::To(0x1235))
        );
    }
    // REP INSW: the emulator walks the count and moves RIP.
    let ins = 0x01F0_0000 | 1 | 1 << 2 | 1 << 3 | 1 << 5 | 1 << 9;
    assert!(matches!(
        run(ExitCode::IOIO, ins, 0x1235, None),
        Action::Vmm(Hand::Pio(Ioio { string: true, .. }), Next::Emulator)
    ));
    // No operand size bit: not what a CPU writes.
    assert_eq!(
        run(ExitCode::IOIO, 0x03F8_0000, 0, None),
        Action::Kill(Kill::BadInfo(ExitCode::IOIO))
    );
}

#[test]
fn exceptions_are_reflected_with_their_error_codes() {
    let x = |v: u64| ExitCode(0x040 + v);
    // #PF: EXITINFO1 the error code, EXITINFO2 the address for CR2.
    assert_eq!(
        run_all(x(14), 0b110, 0xDEAD_B000, None),
        inject(14, Some(0b110), Some(0xDEAD_B000))
    );
    assert_eq!(run_all(x(13), 0x18, 0, None), inject(13, Some(0x18), None));
    assert_eq!(run_all(x(17), 0, 0, None), inject(17, Some(0), None));
    assert_eq!(run_all(x(1), 0xFFFF, 0xFFFF, None), inject(1, None, None));
    assert_eq!(run_all(x(6), 0, 0, None), inject(6, None, None));
    // #DF: EXITINFO1 is undefined, and the code is always 0.
    assert_eq!(run_all(x(8), 0x55, 0, None), inject(8, Some(0), None));
    assert_eq!(
        run_all(x(18), 0, 0, None),
        Action::Kernel(Kernel::MachineCheck, Next::Stay)
    );
    // #BP and #OF save the instruction's own RIP; #TS's info is undefined.
    for v in [3, 4, 10, 29] {
        assert_eq!(
            run_all(x(v), 0, 0, None),
            Action::Kill(Kill::Unsupported(x(v)))
        );
    }
    // An error code past 32 bits, and vectors that are not exceptions.
    assert_eq!(
        run_all(x(13), 1 << 32, 0, None),
        Action::Kill(Kill::BadInfo(x(13)))
    );
    for v in [2, 9, 15, 22, 31] {
        assert_eq!(run_all(x(v), 0, 0, None), Action::Kill(Kill::BadInfo(x(v))));
    }
}

#[test]
fn guest_mode_svm_hypercalls_and_hidden_instructions_are_ud() {
    for code in [
        ExitCode::VMRUN,
        ExitCode::VMLOAD,
        ExitCode::VMSAVE,
        ExitCode::STGI,
        ExitCode::CLGI,
        ExitCode::SKINIT,
        ExitCode::INVLPGA,
        ExitCode::VMMCALL,
        ExitCode::MONITOR,
        ExitCode::MWAIT,
        ExitCode::MWAIT_CONDITIONAL,
        ExitCode::RDPRU,
    ] {
        assert_eq!(
            run(code, 0, 0, Some(3)),
            inject(6, None, None),
            "{}",
            code.name()
        );
    }
    assert_eq!(run(ExitCode::RDPMC, 0, 0, None), inject(13, Some(0), None));
    assert_eq!(
        run(ExitCode::SHUTDOWN, 0, 0, None),
        Action::Kill(Kill::Shutdown)
    );
    assert_eq!(
        run(ExitCode::INVALID, 0, 0, None),
        Action::Kill(Kill::Refused)
    );
    assert_eq!(run(ExitCode::BUSY, 0, 0, None), Action::Kill(Kill::Refused));
}

#[test]
fn nested_faults_are_mmio_outside_ram_and_fatal_inside() {
    const FINAL: u64 = 1 << 32;
    const WRITE: u64 = 1 << 1;
    let npf = |info1: u64, gpa: u64| run(ExitCode::NPF, info1, gpa, None);
    let killed = |info1: u64, gpa: u64| Action::Kill(Kill::Nested(Npf::decode(info1, gpa)));
    // A virtio-mmio or PCI BAR read below 4 GiB, and a write.
    assert_eq!(
        npf(FINAL | 0b100, 0xD000_0000),
        Action::Vmm(
            Hand::Mmio {
                gpa: 0xD000_0000,
                write: false
            },
            Next::Emulator
        )
    );
    assert_eq!(
        npf(FINAL | WRITE | 0b100, 0xD000_0010),
        Action::Vmm(
            Hand::Mmio {
                gpa: 0xD000_0010,
                write: true
            },
            Next::Emulator
        )
    );
    // The APICs' windows are the kernel's.
    for (base, ioapic) in [(IOAPIC, true), (LAPIC, false)] {
        for gpa in [base, base + APIC_WINDOW - 4] {
            assert_eq!(
                npf(FINAL | WRITE, gpa),
                Action::Kernel(
                    Kernel::Apic {
                        gpa,
                        write: true,
                        ioapic
                    },
                    Next::Emulator
                )
            );
        }
    }
    // RAM is mapped in full: a fault in it means the table is wrong.
    assert_eq!(
        npf(FINAL | WRITE | 1, 0x10_0000),
        killed(FINAL | WRITE | 1, 0x10_0000)
    );
    assert_eq!(npf(FINAL, 1536 * MIB - 1), killed(FINAL, 1536 * MIB - 1));
    // A reserved bit, the guest's own walk, a fetch, or past the guest.
    for info1 in [FINAL | 1 << 3, 1 << 33, FINAL | 1 << 4] {
        assert_eq!(npf(info1, 0xD000_0000), killed(info1, 0xD000_0000));
    }
    assert_eq!(npf(FINAL, 4 * GIB), killed(FINAL, 4 * GIB));
    assert_eq!(npf(FINAL, u64::MAX), killed(FINAL, u64::MAX));
}

#[test]
fn a_write_to_rom_is_mmio_and_a_read_of_it_is_fatal() {
    let mut ram = Layout::new(4 * GIB, 2 * MIB).unwrap();
    let rom = Region {
        gpa: 0xFFE0_0000,
        bytes: 2 * MIB,
        offset: 0,
        access: Access::READ,
    };
    ram.add(rom).unwrap();
    let guest = Guest {
        intercepts: intercepts(),
        ram: &ram,
    };
    let write = exit(ExitCode::NPF, 1 << 32 | 1 << 1 | 1, 0xFFE0_1000, None);
    assert_eq!(
        dispatch(&write, &guest),
        Action::Vmm(
            Hand::Mmio {
                gpa: 0xFFE0_1000,
                write: true
            },
            Next::Emulator
        )
    );
    let read = exit(ExitCode::NPF, 1 << 32 | 1, 0xFFE0_1000, None);
    assert!(matches!(
        dispatch(&read, &guest),
        Action::Kill(Kill::Nested(_))
    ));
}

#[test]
fn without_nrips_the_length_is_decoded_or_the_guest_is_killed() {
    let insns = [
        Insn::Cpuid,
        Insn::Rdmsr,
        Insn::Wrmsr,
        Insn::Hlt,
        Insn::Xsetbv,
        Insn::Invd,
        Insn::Wbinvd,
        Insn::Rdtsc,
        Insn::Rdtscp,
    ];
    for insn in insns {
        let op = insn.opcode();
        let mut bytes = op.to_vec();
        bytes.extend([0xCC; 8]);
        assert_eq!(length(insn, &bytes, true), Some(op.len() as u8));
        // Prefixes count: an operand size and a segment override, and REX
        // in 64-bit mode only (else 0x48 is DEC EAX).
        let mut prefixed = vec![0x66, 0x2E];
        prefixed.extend(op);
        assert_eq!(length(insn, &prefixed, false), Some(op.len() as u8 + 2));
        let mut rex = vec![0x48];
        rex.extend(op);
        assert_eq!(length(insn, &rex, true), Some(op.len() as u8 + 1));
        assert_eq!(length(insn, &rex, false), None);
        // Cut short (a page boundary the kernel could not read past).
        assert_eq!(length(insn, &op[..op.len() - 1], true), None);
        // Not the instruction that exited: another vCPU rewrote it.
        assert_eq!(length(insn, &[0x90; 15], true), None);
    }
    // Fifteen bytes at most: fourteen prefixes and HLT fit, fifteen do not.
    let mut long = vec![0x66; 14];
    long.push(0xF4);
    assert_eq!(length(Insn::Hlt, &long, false), Some(15));
    let mut too_long = vec![0x66; 15];
    too_long.push(0xF4);
    assert_eq!(length(Insn::Hlt, &too_long, false), None);
    assert_eq!(length(Insn::Hlt, &[], false), None);
    // CPUID's two bytes are not XSETBV's three.
    assert_eq!(length(Insn::Xsetbv, &[0x0F, 0xA2, 0xD1], true), None);
}

#[test]
fn an_exit_reads_nrip_only_with_nrips() {
    let mut vmcb = Box::new(svm::Vmcb::zeroed());
    vmcb.control.exit_code = ExitCode::CPUID.0;
    vmcb.control.exit_info_1 = 7;
    vmcb.control.exit_info_2 = 8;
    vmcb.control.next_rip = 0x4002;
    assert_eq!(
        Exit::read(&vmcb.control, true),
        exit(ExitCode::CPUID, 7, 8, Some(0x4002))
    );
    assert_eq!(Exit::read(&vmcb.control, false).nrip, None);
}
