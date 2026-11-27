//! The kernel's MSR allow-list: on it, the MSR exit is emulated as
//! `dispatch` said; off it, #GP(0) with RIP unchanged.

mod common;

use common::guest_ram;
use svm::Event;
use svm::exit::{
    Action, Exit, ExitCode, Fault, Guest, Kernel, MSR_ALLOWED, Next, intercepts, msr, msr_allowed,
};

fn at(info1: u64, rcx: u64, nrip: Option<u64>) -> Action {
    let ram = guest_ram();
    let guest = Guest {
        intercepts: intercepts(),
        ram: &ram,
    };
    let exit = Exit {
        code: ExitCode::MSR,
        info1,
        info2: 0,
        nrip,
        interrupted: 0,
    };
    msr(&exit, &guest, rcx)
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
        Action::Kernel(
            Kernel::Msr { write: true },
            Next::Decode(svm::exit::Insn::Wrmsr)
        )
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
}

#[test]
fn other_exits_pass_through_unchanged() {
    let ram = guest_ram();
    let guest = Guest {
        intercepts: intercepts(),
        ram: &ram,
    };
    let cpuid = Exit {
        code: ExitCode::CPUID,
        info1: 0,
        info2: 0,
        nrip: Some(7),
        interrupted: 0,
    };
    assert_eq!(
        msr(&cpuid, &guest, 0xC001_0114),
        svm::exit::dispatch(&cpuid, &guest)
    );
    // A bad MSR exit is still killed for its info, before the list.
    assert!(matches!(at(2, 0xC000_0080, None), Action::Kill(_)));
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
