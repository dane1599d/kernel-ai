//! What the kernel does with an exit (P6 row 3). Everything here is pure:
//! the run loop reads an [`Exit`] out of the VMCB, asks [`dispatch`], and acts.

use super::{ExitCode, Ioio, Npf};

/// One #VMEXIT, as the VMCB reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exit {
    pub code: ExitCode,
    pub info1: u64,
    pub info2: u64,
    /// nRIP (VMCB 0x0C8) when the CPU has NRIPS; None without it, as under
    /// QEMU's TCG (ADR 0006), and the kernel then decodes the instruction's
    /// length itself ([`Next::Decode`]).
    pub nrip: Option<u64>,
    /// EXITINTINFO (VMCB 0x088) with its error code in bits 63:32: the event
    /// the guest was delivering when it exited, if its valid bit is set.
    pub interrupted: u64,
}

impl Exit {
    /// The exit `control` holds; `nrips` is CPUID 0x8000_000A EDX bit 3.
    pub const fn read(control: &crate::ControlArea, nrips: bool) -> Self {
        Self {
            code: ExitCode(control.exit_code),
            info1: control.exit_info_1,
            info2: control.exit_info_2,
            nrip: match nrips {
                true => Some(control.next_rip),
                false => None,
            },
            interrupted: control.exit_int_info as u64 | (control.exit_int_info_error as u64) << 32,
        }
    }
}

/// What the guest's machine is, for the exits that depend on it: the
/// intercepts its VMCB asked for, and where its RAM is.
#[derive(Clone, Copy, Debug)]
pub struct Guest<'a> {
    /// The VMCB's intercepts ([`crate::Intercepts::of`]): an exit it did not
    /// ask for kills the guest.
    pub intercepts: crate::Intercepts,
    /// Guest RAM, mapped in full by the nested table ([`crate::npt::Layout`]).
    pub ram: &'a crate::npt::Layout,
}

/// The IOAPIC's registers, at the base the MP table gives it (Firecracker's
/// and every PC's): the kernel routes its interrupts (ADR 0006 item 3).
pub const IOAPIC: u64 = 0xFEC0_0000;
/// The local APIC's xAPIC registers: the kernel emulates the APIC.
pub const LAPIC: u64 = 0xFEE0_0000;
/// Each APIC window's size.
pub const APIC_WINDOW: u64 = 0x1000;

/// What [`dispatch`] decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// The kernel emulates it, then moves RIP as [`Next`] says.
    Kernel(Kernel, Next),
    /// It goes to `vmm` on the VMEXIT ring (ADR 0006 item 3), and RIP moves
    /// as [`Next`] says once `vmm` answers.
    Vmm(Hand, Next),
    /// A fault for the guest: injected at the next VMRUN, RIP unchanged.
    Inject(Fault),
    /// The exit came while the guest was delivering this event (EXITINTINFO,
    /// APM 15.7.2): the host takes its own interrupt, then injects the event
    /// again at the next VMRUN, RIP unchanged, so the guest does not lose it.
    Redeliver(crate::Event),
    /// The guest cannot go on: the kernel stops its vCPUs and tells `vmm`.
    Kill(Kill),
}

/// What the kernel emulates itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kernel {
    /// A host interrupt, NMI, SMI or INIT: the host takes it after STGI,
    /// and the guest runs again where it was.
    HostEvent,
    /// A machine check in the guest: the host's #MC handler runs (APM 15.12.18).
    MachineCheck,
    /// The guest can take an interrupt now (VINTR).
    InterruptWindow,
    /// CPUID, from the table fixed at create.
    Cpuid,
    /// RDMSR or WRMSR on the kernel's allow-list; any other MSR is #GP(0).
    Msr { write: bool },
    /// HLT: the vCPU sleeps until an interrupt for it.
    Hlt,
    /// XSETBV: XCR0 checked against what CPUID offers.
    Xsetbv,
    /// RDTSC or RDTSCP, with the VM's TSC offset.
    Rdtsc { rdtscp: bool },
    /// INVD or WBINVD: the kernel writes caches back (never drops them).
    Flush,
    /// PAUSE in a spin loop: the vCPU yields, and RIP steps past it. An
    /// intercept fires before the instruction runs, so a PAUSE left at RIP
    /// would exit again for ever: TCG has no pause filter, and a zero
    /// filter count exits on every PAUSE on hardware too (APM 15.14.4).
    Pause,
    /// An access to the IOAPIC's or local APIC's registers.
    Apic { gpa: u64, write: bool, ioapic: bool },
}

/// What goes to `vmm`: the ring's Pio and Mmio kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    Pio(Ioio),
    /// An access to guest physical memory that is not RAM. The kernel's
    /// instruction emulator (ADR 0006 item 6) finds its size and data.
    Mmio {
        gpa: u64,
        write: bool,
    },
}

/// A fault to inject into the guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault {
    pub event: crate::Event,
    /// A #PF's faulting address, for the guest's CR2 (the intercept fires
    /// before CR2 is written, APM 15.12.15).
    pub cr2: Option<u64>,
}

/// Why the guest is killed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kill {
    /// A shutdown or triple fault (APM 15.14.3: the guest's state is undefined).
    Shutdown,
    /// VMRUN refused the VMCB, or a busy VMSA: the kernel's own error.
    Refused,
    /// An exit code `svm` does not name.
    Unknown(ExitCode),
    /// An exit for an intercept the VMCB did not ask for.
    Unrequested(ExitCode),
    /// An exit the kernel asks for nowhere and has no emulation of:
    /// CR and DR accesses, task switches, SEV-ES and AVIC exits.
    Unsupported(ExitCode),
    /// EXITINFO1 or EXITINFO2 holds what the CPU never writes.
    BadInfo(ExitCode),
    /// A nested fault the table should never give: inside RAM, on a
    /// reserved bit, during the guest's own table walk, or a fetch from MMIO.
    Nested(Npf),
    /// Without NRIPS, the bytes at RIP are not the instruction that exited
    /// ([`length`] refused them).
    Undecodable,
    /// The exit came while the guest was delivering an event, and the kernel
    /// cannot finish that delivery: a #NPF on the IDT or stack (in MMIO), an
    /// exception intercept, or a software interrupt, which needs the INTn's
    /// length to inject again.
    Delivery(ExitCode),
}

/// How RIP moves once the exit is handled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// It stays: the guest runs the same instruction again, or was where it
    /// should be.
    Stay,
    /// It moves to this address: nRIP, or an IN or OUT's EXITINFO2.
    To(u64),
    /// No NRIPS: the kernel reads up to 15 bytes at the guest's CS:RIP and
    /// [`length`] says how far to move, or [`Kill::Undecodable`].
    Decode(Insn),
    /// The instruction emulator moves RIP itself: MMIO and string I/O, whose
    /// length it decodes in any case (#NPF saves no nRIP, APM 15.7.1).
    Emulator,
}

/// The instructions the kernel emulates and steps past.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Insn {
    Cpuid,
    Rdmsr,
    Wrmsr,
    Hlt,
    Xsetbv,
    Invd,
    Wbinvd,
    Rdtsc,
    Rdtscp,
    /// F3 90, with F3 among the prefixes and no REX.B.
    Pause,
}

impl Insn {
    /// The opcode bytes, after any prefixes (APM vol. 3).
    pub const fn opcode(self) -> &'static [u8] {
        match self {
            Self::Cpuid => &[0x0F, 0xA2],
            Self::Rdmsr => &[0x0F, 0x32],
            Self::Wrmsr => &[0x0F, 0x30],
            Self::Hlt => &[0xF4],
            Self::Xsetbv => &[0x0F, 0x01, 0xD1],
            Self::Invd => &[0x0F, 0x08],
            Self::Wbinvd => &[0x0F, 0x09],
            Self::Rdtsc => &[0x0F, 0x31],
            Self::Rdtscp => &[0x0F, 0x01, 0xF9],
            // The F3 is a prefix as far as the decoder goes; `length` asks
            // for it.
            Self::Pause => &[0x90],
        }
    }
}

/// The longest x86 instruction.
pub const MAX_INSN: usize = 15;

/// The length of `insn` in `bytes`, read at the guest's RIP: its prefixes
/// (legacy ones, and REX in 64-bit mode, `long`), then its opcode. PAUSE is
/// 90 after an F3 that is the last of the F2 and F3 prefixes, with no REX.B
/// on the 90 (REX.B 90 is XCHG r8, rAX). None when
/// the bytes are not that instruction, are cut short, or run past 15 bytes;
/// the guest is then killed rather than stepped to a guessed RIP. Another
/// vCPU can rewrite the bytes after the exit, so they are checked, not
/// trusted.
pub fn length(insn: Insn, bytes: &[u8], long: bool) -> Option<u8> {
    let bytes = &bytes[..bytes.len().min(MAX_INSN)];
    let at = bytes
        .iter()
        .position(|&b| !(legacy_prefix(b) || long && b & 0xF0 == 0x40))?;
    let end = at + insn.opcode().len();
    if insn == Insn::Pause {
        let rep = bytes[..at].iter().rev().find(|&&b| b == 0xF2 || b == 0xF3);
        // Only a REX right before the opcode counts (APM vol. 3, 1.2.7).
        let rex_b = long && at > 0 && bytes[at - 1] & 0xF1 == 0x41;
        if rep != Some(&0xF3) || rex_b {
            return None;
        }
    }
    match bytes.get(at..end) {
        Some(op) if op == insn.opcode() => Some(end as u8),
        _ => None,
    }
}

/// Operand and address size, LOCK, REP and segment prefixes.
const fn legacy_prefix(b: u8) -> bool {
    matches!(
        b,
        0x66 | 0x67 | 0xF0 | 0xF2 | 0xF3 | 0x26 | 0x2E | 0x36 | 0x3E | 0x64 | 0x65
    )
}

/// The intercept bit that asks for `code`, if one does: NPF comes from
/// NP_ENABLE, and AVIC, VMGEXIT, INVALID and BUSY from none.
pub const fn intercept(code: ExitCode) -> Option<crate::Intercept> {
    use crate::Intercept;
    match code.0 {
        0x000..=0x00F => Some(Intercept::cr_read(code.0 as u8)),
        0x010..=0x01F => Some(Intercept::cr_write((code.0 - 0x010) as u8)),
        0x020..=0x02F => Some(Intercept::dr_read((code.0 - 0x020) as u8)),
        0x030..=0x03F => Some(Intercept::dr_write((code.0 - 0x030) as u8)),
        0x040..=0x05F => Some(Intercept::exception((code.0 - 0x040) as u8)),
        // From INTR (word 3 bit 0) to IDLE_HLT (word 5 bit 6), exit codes and
        // intercept bits run in the same order (APM table C-1 and B-1).
        0x060..=0x0A6 => Some(Intercept((code.0 - 0x060 + 96) as u16)),
        _ => None,
    }
}

/// The intercepts a guest runs with: [`crate::Intercepts::required`], the
/// instructions [`dispatch`] emulates or answers with #UD, and #DB, #AC and
/// #MC. #DB and #AC are reflected back, so a guest cannot hang the CPU in an
/// endless chain of them (CVE-2015-8104, CVE-2015-5307). VINTR is added
/// only while the kernel waits for an interrupt window. RDTSC is left to the
/// CPU with the VMCB's TSC offset, which needs no exit; RDTSCP is taken, so
/// the guest reads its own TSC_AUX, not the host's CPU number.
pub const fn intercepts() -> crate::Intercepts {
    use crate::Intercept;
    crate::Intercepts::required()
        .with(Intercept::INIT)
        .with(Intercept::CPUID)
        .with(Intercept::HLT)
        .with(Intercept::PAUSE)
        .with(Intercept::INVD)
        .with(Intercept::WBINVD)
        .with(Intercept::XSETBV)
        .with(Intercept::RDTSCP)
        .with(Intercept::RDPMC)
        .with(Intercept::INVLPGA)
        .with(Intercept::VMMCALL)
        .with(Intercept::MONITOR)
        .with(Intercept::MWAIT)
        .with(Intercept::MWAIT_CONDITIONAL)
        .with(Intercept::RDPRU)
        .with(Intercept::exception(1))
        .with(Intercept::exception(17))
        .with(Intercept::exception(18))
}

/// What the kernel does with `exit`. Every code maps to one action, and
/// nothing in an exit makes it panic: an unknown code, an intercept the
/// VMCB did not set, or info words the CPU never writes kill the guest.
pub fn dispatch(exit: &Exit, guest: &Guest) -> Action {
    let code = exit.code;
    if code.name() == "unknown" {
        return Action::Kill(Kill::Unknown(code));
    }
    if let Some(bit) = intercept(code)
        && !guest.intercepts.contains(bit)
    {
        return Action::Kill(Kill::Unrequested(code));
    }
    let next = |insn| match exit.nrip {
        Some(rip) => Next::To(rip),
        None => Next::Decode(insn),
    };
    let kernel = |what, insn| Action::Kernel(what, next(insn));
    match crate::Event::decode(exit.interrupted) {
        Err(_) => return Action::Kill(Kill::BadInfo(code)),
        Ok(Some(event)) => return delivering(code, event),
        Ok(None) => {}
    }
    match code {
        ExitCode::INTR | ExitCode::NMI | ExitCode::SMI | ExitCode::INIT => {
            Action::Kernel(Kernel::HostEvent, Next::Stay)
        }
        ExitCode::VINTR => Action::Kernel(Kernel::InterruptWindow, Next::Stay),
        ExitCode::CPUID => kernel(Kernel::Cpuid, Insn::Cpuid),
        ExitCode::HLT | ExitCode::IDLE_HLT => kernel(Kernel::Hlt, Insn::Hlt),
        ExitCode::PAUSE => kernel(Kernel::Pause, Insn::Pause),
        ExitCode::XSETBV => kernel(Kernel::Xsetbv, Insn::Xsetbv),
        ExitCode::INVD => kernel(Kernel::Flush, Insn::Invd),
        ExitCode::WBINVD => kernel(Kernel::Flush, Insn::Wbinvd),
        ExitCode::RDTSC => kernel(Kernel::Rdtsc { rdtscp: false }, Insn::Rdtsc),
        ExitCode::RDTSCP => kernel(Kernel::Rdtsc { rdtscp: true }, Insn::Rdtscp),
        ExitCode::MSR => match exit.info1 {
            0 => kernel(Kernel::Msr { write: false }, Insn::Rdmsr),
            1 => kernel(Kernel::Msr { write: true }, Insn::Wrmsr),
            _ => Action::Kill(Kill::BadInfo(code)),
        },
        ExitCode::IOIO => ioio(exit),
        ExitCode::NPF => npf(exit, guest),
        // No PMU is offered: RDPMC is #GP(0), as with no counter at ECX.
        ExitCode::RDPMC => fault(code, 13, Some(0), None),
        // No nested SVM, no hypercalls, and CPUID hides MONITOR and RDPRU.
        ExitCode::VMRUN
        | ExitCode::VMLOAD
        | ExitCode::VMSAVE
        | ExitCode::STGI
        | ExitCode::CLGI
        | ExitCode::SKINIT
        | ExitCode::INVLPGA
        | ExitCode::VMMCALL
        | ExitCode::MONITOR
        | ExitCode::MWAIT
        | ExitCode::MWAIT_CONDITIONAL
        | ExitCode::RDPRU => fault(code, 6, None, None),
        ExitCode::SHUTDOWN => Action::Kill(Kill::Shutdown),
        ExitCode::INVALID | ExitCode::BUSY => Action::Kill(Kill::Refused),
        _ => match code.exception() {
            Some(vector) => exception(exit, vector),
            None => Action::Kill(Kill::Unsupported(code)),
        },
    }
}

/// An exit taken while the guest was delivering `event`. Only an interrupt,
/// NMI, SMI or INIT of the host's own can arrive then with nothing of the
/// guest's to finish, so the event is injected again; a shutdown and a
/// refused VMCB stay what they are; anything else kills the guest.
fn delivering(code: ExitCode, event: crate::Event) -> Action {
    match code {
        ExitCode::SHUTDOWN => Action::Kill(Kill::Shutdown),
        ExitCode::INVALID | ExitCode::BUSY => Action::Kill(Kill::Refused),
        ExitCode::INTR | ExitCode::NMI | ExitCode::SMI | ExitCode::INIT => match event.kind {
            crate::EventType::Software => Action::Kill(Kill::Delivery(code)),
            // An exception is checked as an injected one is, so VMRUN never
            // refuses what goes back.
            crate::EventType::Exception => {
                match crate::Event::exception(event.vector, event.error_code) {
                    Ok(event) => Action::Redeliver(event),
                    Err(_) => Action::Kill(Kill::BadInfo(code)),
                }
            }
            _ => Action::Redeliver(event),
        },
        _ => Action::Kill(Kill::Delivery(code)),
    }
}

/// An IN or OUT: EXITINFO2 is the next RIP with or without NRIPS (APM
/// 15.10.2). A string access moves RIP through the emulator, which walks
/// its count and addresses.
fn ioio(exit: &Exit) -> Action {
    match Ioio::decode(exit.info1) {
        Some(io) if io.string => Action::Vmm(Hand::Pio(io), Next::Emulator),
        Some(io) => Action::Vmm(Hand::Pio(io), Next::To(exit.info2)),
        None => Action::Kill(Kill::BadInfo(exit.code)),
    }
}

/// A nested page fault. RAM is mapped in full with its rights at create,
/// so a fault in it, on a reserved bit, or on the guest's own page tables
/// is the table's bug or a hostile guest: the guest is killed. A data
/// access elsewhere below the guest's size is MMIO; an APIC's goes to the
/// kernel, the rest to `vmm`. A write to read-only memory (a ROM) is MMIO
/// too. Past the guest's size there is nothing to emulate but the APICs.
/// EXITINFO1 with both or neither of bits 32 and 33 is not one the CPU
/// writes.
fn npf(exit: &Exit, guest: &Guest) -> Action {
    let fault = Npf::decode(exit.info1, exit.info2);
    let gpa = fault.gpa;
    if fault.final_address == fault.page_table {
        return Action::Kill(Kill::BadInfo(exit.code));
    }
    let apic = |base: u64| gpa >= base && gpa - base < APIC_WINDOW;
    let on_apic = apic(IOAPIC) || apic(LAPIC);
    let rom_write = guest
        .ram
        .find(gpa)
        .is_some_and(|region| fault.write && !region.access.write);
    let in_ram = guest.ram.find(gpa).is_some() && !rom_write;
    let past = gpa >= guest.ram.guest() && !on_apic;
    if in_ram || fault.reserved || fault.page_table || fault.fetch || past {
        return Action::Kill(Kill::Nested(fault));
    }
    if on_apic {
        let what = Kernel::Apic {
            gpa,
            write: fault.write,
            ioapic: apic(IOAPIC),
        };
        return Action::Kernel(what, Next::Emulator);
    }
    Action::Vmm(
        Hand::Mmio {
            gpa,
            write: fault.write,
        },
        Next::Emulator,
    )
}

/// An intercepted exception, reflected into the guest with the error code
/// the CPU gave in EXITINFO1 (APM 15.12). #MC is the host's. #BP and #OF
/// save the INT3 or INTO's own RIP, so a reflected trap would loop; #TS
/// and #VC have no defined EXITINFO1. Those, and vectors that are not
/// exceptions, kill the guest.
fn exception(exit: &Exit, vector: u8) -> Action {
    let code = exit.code;
    match vector {
        18 => Action::Kernel(Kernel::MachineCheck, Next::Stay),
        3 | 4 | 10 | 29 => Action::Kill(Kill::Unsupported(code)),
        // #DF's error code is always 0; EXITINFO1 is undefined.
        8 => fault(code, 8, Some(0), None),
        14 => match u32::try_from(exit.info1) {
            Ok(error) => fault(code, 14, Some(error), Some(exit.info2)),
            Err(_) => Action::Kill(Kill::BadInfo(code)),
        },
        _ if crate::event::pushes_error_code(vector) => match u32::try_from(exit.info1) {
            Ok(error) => fault(code, vector, Some(error), None),
            Err(_) => Action::Kill(Kill::BadInfo(code)),
        },
        _ => fault(code, vector, None, None),
    }
}

/// Exception `vector` as an action; a vector that is not one kills the
/// guest, so no exit becomes an EVENTINJ the CPU would refuse.
fn fault(code: ExitCode, vector: u8, error: Option<u32>, cr2: Option<u64>) -> Action {
    match crate::Event::exception(vector, error) {
        Ok(event) => Action::Inject(Fault { event, cr2 }),
        Err(_) => Action::Kill(Kill::BadInfo(code)),
    }
}
