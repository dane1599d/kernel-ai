//! The kernel's MSR allow-list, as `dispatch` applies it from the guest's
//! RCX: on the list, the MSR exit is emulated; off it, #GP(0) with RIP
//! unchanged. There is no other way to `Kernel::Msr`.

mod common;

use common::guest_ram;
use svm::Event;
use svm::exit::{
    Action, Exit, ExitCode, Fault, Guest, Insn, Kernel, MSR_ALLOWED, Next, dispatch, intercepts,
    msr_allowed,
};

fn exit(code: ExitCode, info1: u64, rcx: u64, nrip: Option<u64>) -> Exit {
    Exit {
        code,
        info1,
        info2: 0,
        nrip,
        interrupted: 0,
        rcx,
    }
}

fn at(info1: u64, rcx: u64, nrip: Option<u64>) -> Action {
    let ram = guest_ram();
    let guest = Guest {
        intercepts: intercepts(),
        ram: &ram,
    };
    dispatch(&exit(ExitCode::MSR, info1, rcx, nrip), &guest)
}

fn gp0() -> Action {
    Action::Inject(Fault {
        event: Event::exception(13, Some(0)).unwrap(),
        cr2: None,
    })
}

#[test]
fn an_msr_on_the_list_is_emulated() {
    const EFER: u64 = 0xC000_0080;
    assert_eq!(
        at(0, EFER, Some(0x1002)),
        Action::Kernel(Kernel::Msr { write: false }, Next::To(0x1002))
    );
    assert_eq!(
        at(1, 0x830, None),
        Action::Kernel(Kernel::Msr { write: true }, Next::Decode(Insn::Wrmsr))
    );
    // RDMSR and WRMSR take ECX: RCX's upper half does not count.
    assert_eq!(
        at(0, 0xDEAD_0000_0000_0000 | EFER, Some(2)),
        Action::Kernel(Kernel::Msr { write: false }, Next::To(2))
    );
}

#[test]
fn an_msr_off_the_list_is_gp0() {
    // VM_CR and VM_HSAVE_PA: the guest gets no SVM of its own.
    assert_eq!(at(0, 0xC001_0114, Some(2)), gp0());
    assert_eq!(at(1, 0xC001_0117, Some(2)), gp0());
    // A PMU counter, with no PMU offered.
    assert_eq!(at(0, 0xC001_0004, None), gp0());
    // MTRRCAP and MISC_ENABLE are read-only.
    assert_eq!(at(1, 0xFE, Some(2)), gp0());
    assert_eq!(at(1, 0x1A0, Some(2)), gp0());
    assert!(msr_allowed(0xFE, false) && !msr_allowed(0xFE, true));
    // Just past the x2APIC's range.
    assert!(msr_allowed(0x8FF, true) && !msr_allowed(0x900, false));
    // A bad MSR exit is still killed for its info, before the list.
    assert!(matches!(at(2, 0xC000_0080, None), Action::Kill(_)));
}

#[test]
fn rcx_matters_only_to_an_msr_exit() {
    let ram = guest_ram();
    let guest = Guest {
        intercepts: intercepts(),
        ram: &ram,
    };
    let off_list = dispatch(&exit(ExitCode::CPUID, 0, 0xC001_0114, Some(7)), &guest);
    let on_list = dispatch(&exit(ExitCode::CPUID, 0, 0xC000_0080, Some(7)), &guest);
    assert_eq!(off_list, on_list);
    assert_eq!(off_list, Action::Kernel(Kernel::Cpuid, Next::To(7)));
}

#[test]
fn the_list_is_sorted_and_its_ranges_do_not_overlap() {
    for pair in MSR_ALLOWED.windows(2) {
        assert!(pair[0].first <= pair[0].last);
        assert!(
            pair[0].last < pair[1].first,
            "{} and {}",
            pair[0].name,
            pair[1].name
        );
    }
    assert!(MSR_ALLOWED.iter().all(|a| a.read));
}
