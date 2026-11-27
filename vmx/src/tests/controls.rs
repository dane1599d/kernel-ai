//! Control bits against vol. 3C tables 25-5 to 25-7, 25-13 and 25-15, and
//! their fit to the capability MSRs (vol. 3D, A.3).

use crate::controls::{
    Capabilities, Controls, Refusal, Word, entry, exit, pin, primary, secondary,
};

#[test]
fn control_bits_are_the_tables_positions() {
    let table: &[(u32, u32)] = &[
        (pin::EXTERNAL_INTERRUPT_EXITING, 0),
        (pin::NMI_EXITING, 3),
        (pin::VIRTUAL_NMIS, 5),
        (pin::PREEMPTION_TIMER, 6),
        (primary::HLT_EXITING, 7),
        (primary::CR3_LOAD_EXITING, 15),
        (primary::MOV_DR_EXITING, 23),
        (primary::UNCONDITIONAL_IO_EXITING, 24),
        (primary::IO_BITMAPS, 25),
        (primary::MSR_BITMAPS, 28),
        (primary::SECONDARY_CONTROLS, 31),
        (secondary::EPT, 1),
        (secondary::VPID, 5),
        (secondary::UNRESTRICTED_GUEST, 7),
        (secondary::VMCS_SHADOWING, 14),
        (secondary::MODE_BASED_EPT_EXECUTE, 22),
        (secondary::INSTRUCTION_TIMEOUT, 31),
        (exit::HOST_ADDRESS_SPACE_SIZE, 9),
        (exit::ACKNOWLEDGE_INTERRUPT, 15),
        (exit::SAVE_PAT, 18),
        (exit::LOAD_EFER, 21),
        (entry::IA32E_MODE_GUEST, 9),
        (entry::LOAD_PAT, 14),
        (entry::LOAD_EFER, 15),
    ];
    for &(bit, position) in table {
        assert_eq!(bit, 1 << position);
    }
}

#[test]
fn the_minimum_passes_and_each_part_of_it_is_needed() {
    let need = Controls::required();
    assert_eq!(need.check(), Ok(()));
    assert_eq!(need.exceptions, 1 << 1 | 1 << 17 | 1 << 18);
    let missing = Controls {
        secondary: 0,
        ..need
    };
    assert_eq!(
        missing.check(),
        Err(Refusal::BelowMinimum(Word::Secondary, secondary::EPT))
    );
    let no_io = Controls {
        primary: primary::SECONDARY_CONTROLS,
        ..need
    };
    assert_eq!(
        no_io.check(),
        Err(Refusal::BelowMinimum(Word::Primary, primary::IO_BITMAPS))
    );
    let no_nmi = Controls {
        pin: pin::EXTERNAL_INTERRUPT_EXITING,
        ..need
    };
    assert_eq!(
        no_nmi.check(),
        Err(Refusal::BelowMinimum(Word::Pin, pin::NMI_EXITING))
    );
    let no_efer = Controls {
        exit: need.exit & !exit::LOAD_EFER,
        ..need
    };
    assert_eq!(
        no_efer.check(),
        Err(Refusal::BelowMinimum(Word::Exit, exit::LOAD_EFER))
    );
    let no_db = Controls {
        exceptions: 1 << 17 | 1 << 18,
        ..need
    };
    assert_eq!(no_db.check(), Err(Refusal::ExceptionsBelowMinimum(1 << 1)));
    assert_eq!(
        Controls::none().check(),
        Err(Refusal::BelowMinimum(Word::Pin, need.pin))
    );
    // More than the minimum is fine.
    let more = need
        .with(Controls {
            primary: primary::HLT_EXITING,
            ..Controls::none()
        })
        .with_exception(14);
    assert_eq!(more.check(), Ok(()));
    assert!(more.exceptions & 1 << 14 != 0);
    assert_eq!(more.with_exception(32), more);
}

/// A capability MSR: `must` in the allowed-0 half, `may` in the allowed-1.
fn cap(must: u32, may: u32) -> u64 {
    (may as u64) << 32 | must as u64
}

fn caps() -> Capabilities {
    Capabilities {
        // The default-1 bits a CPU without TRUE_ MSRs reports (A.2).
        pin: cap(0x16, 0xFF),
        primary: cap(0x0401_E172, u32::MAX),
        secondary: cap(0, 0x00FF_FFFF),
        exit: cap(0x0003_6DFF, u32::MAX),
        entry: cap(0x0000_11FF, u32::MAX),
    }
}

#[test]
fn adjust_adds_the_fixed_ones_and_keeps_what_was_asked() {
    let want = Controls::required();
    let fitted = want.adjust(&caps()).unwrap();
    assert_eq!(fitted.pin, want.pin | 0x16);
    assert_eq!(fitted.primary, want.primary | 0x0401_E172);
    assert_eq!(fitted.secondary, want.secondary);
    assert_eq!(fitted.exit & want.exit, want.exit);
    assert_eq!(fitted.exceptions, want.exceptions);
    assert_eq!(fitted.check(), Ok(()));
}

#[test]
fn adjust_refuses_what_the_cpu_cannot_do() {
    // No EPT on this CPU: the set is refused, not run without it.
    let no_ept = Capabilities {
        secondary: cap(0, !secondary::EPT),
        ..caps()
    };
    assert_eq!(
        Controls::required().adjust(&no_ept),
        Err(Refusal::Unsupported(Word::Secondary, secondary::EPT))
    );
    // A capability MSR that fixes a bit to 1 and forbids it: not trusted.
    let contradictory = Capabilities {
        pin: cap(1 << 4, !(1 << 4)),
        ..caps()
    };
    assert_eq!(
        Controls::required().adjust(&contradictory),
        Err(Refusal::Unsupported(Word::Pin, 1 << 4))
    );
    // All zero: nothing may be 1.
    let zero = Capabilities {
        pin: 0,
        primary: 0,
        secondary: 0,
        exit: 0,
        entry: 0,
    };
    assert_eq!(
        Controls::required().adjust(&zero),
        Err(Refusal::Unsupported(Word::Pin, Controls::required().pin))
    );
    // Secondary controls without the primary bit that activates them: the
    // CPU would ignore them, EPT and all.
    let inactive = Controls {
        primary: primary::IO_BITMAPS,
        ..Controls::required()
    };
    assert_eq!(
        inactive.check(),
        Err(Refusal::BelowMinimum(
            Word::Primary,
            primary::SECONDARY_CONTROLS
        ))
    );
}

/// The capability MSRs of the Intel models the nesting census read under KVM
/// (g16-vmx-census.md, runs 45845412730): the TRUE controls (VMX_BASIC bit
/// 55 was set), and the plain ones, whose fixed-1 bits differ.
struct Census {
    name: &'static str,
    plain: Capabilities,
    true_controls: Capabilities,
}

fn census() -> [Census; 2] {
    // Exit, entry and the secondary word are the same on both models.
    let exit = 0x007F_FFFF_0003_6DFF;
    let entry = 0x0000_F3FF_0000_11FF;
    let true_exit = 0x007F_FFFF_0003_6DFB;
    let true_entry = 0x0000_F3FF_0000_11FB;
    let model = |name, pin, proc, true_proc, proc2| Census {
        name,
        plain: Capabilities {
            pin,
            primary: proc,
            secondary: proc2,
            exit,
            entry,
        },
        true_controls: Capabilities {
            pin,
            primary: true_proc,
            secondary: proc2,
            exit: true_exit,
            entry: true_entry,
        },
    };
    [
        model(
            "Xeon 6975P-C",
            0x00FF_0000_0016,
            0xFFF9_FFFE_0401_E172,
            0xFFF9_FFFE_0400_6172,
            0x0613_7BFF_0000_0000,
        ),
        model(
            "Xeon 8573C",
            0x007F_0000_0016,
            0xF7F9_FFFE_0401_E172,
            0xF7F9_FFFE_0400_6172,
            0x0210_58AE_0000_0000,
        ),
    ]
}

/// The G16 census's fitted words, as `adjust()` vectors: both models give the
/// same ones, from the plain MSRs and from the TRUE ones.
#[test]
fn adjust_fits_the_minimum_to_each_intel_model_the_census_read() {
    let need = Controls::required();
    for model in census() {
        let name = model.name;
        let plain = need
            .adjust(&model.plain)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(
            (
                plain.pin,
                plain.primary,
                plain.secondary,
                plain.exit,
                plain.entry
            ),
            (0x1f, 0x8601_e172, 0x2, 0x3f_6fff, 0xd1ff),
            "{name}, plain MSRs"
        );
        let fitted = need
            .adjust(&model.true_controls)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(
            (
                fitted.pin,
                fitted.primary,
                fitted.secondary,
                fitted.exit,
                fitted.entry
            ),
            (0x1f, 0x8600_6172, 0x2, 0x3f_6ffb, 0xd1fb),
            "{name}, TRUE MSRs"
        );
        assert_eq!(fitted.exceptions, need.exceptions, "{name}");
        assert_eq!(fitted.check(), Ok(()), "{name}");
    }
}

/// The 8573C lacks controls the 6975P-C has (EPT A/D is an EPT_VPID_CAP bit,
/// checked with the EPT pointer): the minimum asks for none of them, so it
/// fits both.
#[test]
fn the_minimum_asks_for_nothing_the_8573c_lacks() {
    let need = Controls::required();
    let [xeon_6975, xeon_8573] = census();
    for caps in [xeon_8573.plain, xeon_8573.true_controls] {
        // Every bit of every word is one the model may set to 1.
        for (word, bits, cap) in [
            (Word::Pin, need.pin, caps.pin),
            (Word::Primary, need.primary, caps.primary),
            (Word::Secondary, need.secondary, caps.secondary),
            (Word::Exit, need.exit, caps.exit),
            (Word::Entry, need.entry, caps.entry),
        ] {
            assert_eq!(bits & !((cap >> 32) as u32), 0, "{word:?}");
        }
    }
    // What the two models differ in, by name and by position (SDM tables
    // 25-5 to 25-7): the minimum has none of it. (The census's list of the
    // 8573C's gaps leaves out RDSEED exiting, bit 16, which its MSR lacks.)
    let [p, d] = [xeon_6975.true_controls, xeon_8573.true_controls].map(|c| {
        [
            (c.pin >> 32) as u32,
            (c.primary >> 32) as u32,
            (c.secondary >> 32) as u32,
        ]
    });
    let lacking = [p[0] & !d[0], p[1] & !d[1], p[2] & !d[2]];
    assert_eq!(lacking[0], pin::POSTED_INTERRUPTS);
    assert_eq!(lacking[1], primary::MONITOR_TRAP_FLAG);
    let secondary_lacking = secondary::VIRTUALIZE_APIC_ACCESSES
        | secondary::VIRTUALIZE_X2APIC
        | secondary::WBINVD_EXITING
        | secondary::APIC_REGISTER_VIRTUALIZATION
        | secondary::VIRTUAL_INTERRUPT_DELIVERY
        | secondary::VM_FUNCTIONS
        | secondary::RDSEED_EXITING
        | secondary::PML
        | secondary::USER_WAIT_PAUSE;
    assert_eq!(lacking[2], secondary_lacking);
    assert_eq!(need.pin & lacking[0], 0);
    assert_eq!(need.primary & lacking[1], 0);
    assert_eq!(need.secondary & lacking[2], 0);
    // And the minimum is an Ok on the model without them.
    assert!(need.adjust(&xeon_8573.true_controls).is_ok());
}
