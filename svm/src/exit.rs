//! `EXITCODE` (VMCB 0x070) and the exit information the kernel's own exits
//! carry in `EXITINFO1`/`EXITINFO2` (APM vol. 2, 15.6, 15.10, 15.11, 15.25.6
//! and appendix C).

/// Why the guest exited: VMCB `exit_code`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExitCode(pub u64);

impl ExitCode {
    /// Reads of CR0-CR15: `CR_READ + n`.
    pub const CR_READ: Self = Self(0x000);
    /// Writes of CR0-CR15: `CR_WRITE + n`.
    pub const CR_WRITE: Self = Self(0x010);
    /// Reads of DR0-DR15.
    pub const DR_READ: Self = Self(0x020);
    /// Writes of DR0-DR15.
    pub const DR_WRITE: Self = Self(0x030);
    /// Exception vector 0-31: `EXCEPTION + vector`.
    pub const EXCEPTION: Self = Self(0x040);
    pub const INTR: Self = Self(0x060);
    pub const NMI: Self = Self(0x061);
    pub const SMI: Self = Self(0x062);
    pub const INIT: Self = Self(0x063);
    pub const VINTR: Self = Self(0x064);
    pub const CR0_SEL_WRITE: Self = Self(0x065);
    pub const IDTR_READ: Self = Self(0x066);
    pub const GDTR_READ: Self = Self(0x067);
    pub const LDTR_READ: Self = Self(0x068);
    pub const TR_READ: Self = Self(0x069);
    pub const IDTR_WRITE: Self = Self(0x06A);
    pub const GDTR_WRITE: Self = Self(0x06B);
    pub const LDTR_WRITE: Self = Self(0x06C);
    pub const TR_WRITE: Self = Self(0x06D);
    pub const RDTSC: Self = Self(0x06E);
    pub const RDPMC: Self = Self(0x06F);
    pub const PUSHF: Self = Self(0x070);
    pub const POPF: Self = Self(0x071);
    pub const CPUID: Self = Self(0x072);
    pub const RSM: Self = Self(0x073);
    pub const IRET: Self = Self(0x074);
    /// INTn (a software interrupt).
    pub const SWINT: Self = Self(0x075);
    pub const INVD: Self = Self(0x076);
    pub const PAUSE: Self = Self(0x077);
    pub const HLT: Self = Self(0x078);
    pub const INVLPG: Self = Self(0x079);
    pub const INVLPGA: Self = Self(0x07A);
    /// IN, OUT, INS or OUTS on a port the IOPM intercepts ([`Ioio`]).
    pub const IOIO: Self = Self(0x07B);
    /// RDMSR or WRMSR on an MSR the MSRPM intercepts ([`msr_write`]).
    pub const MSR: Self = Self(0x07C);
    pub const TASK_SWITCH: Self = Self(0x07D);
    pub const FERR_FREEZE: Self = Self(0x07E);
    /// A triple fault, or a shutdown: the guest cannot go on.
    pub const SHUTDOWN: Self = Self(0x07F);
    pub const VMRUN: Self = Self(0x080);
    pub const VMMCALL: Self = Self(0x081);
    pub const VMLOAD: Self = Self(0x082);
    pub const VMSAVE: Self = Self(0x083);
    pub const STGI: Self = Self(0x084);
    pub const CLGI: Self = Self(0x085);
    pub const SKINIT: Self = Self(0x086);
    pub const RDTSCP: Self = Self(0x087);
    pub const ICEBP: Self = Self(0x088);
    pub const WBINVD: Self = Self(0x089);
    pub const MONITOR: Self = Self(0x08A);
    pub const MWAIT: Self = Self(0x08B);
    pub const MWAIT_CONDITIONAL: Self = Self(0x08C);
    pub const XSETBV: Self = Self(0x08D);
    pub const RDPRU: Self = Self(0x08E);
    /// Trap after the guest wrote EFER.
    pub const EFER_WRITE_TRAP: Self = Self(0x08F);
    /// Traps after writes of CR0-CR15: `CR_WRITE_TRAP + n`.
    pub const CR_WRITE_TRAP: Self = Self(0x090);
    pub const INVPCID: Self = Self(0x0A2);
    pub const BUS_LOCK: Self = Self(0x0A5);
    pub const IDLE_HLT: Self = Self(0x0A6);
    /// A nested page fault ([`Npf`]).
    pub const NPF: Self = Self(0x400);
    pub const AVIC_INCOMPLETE_IPI: Self = Self(0x401);
    pub const AVIC_NOACCEL: Self = Self(0x402);
    pub const VMGEXIT: Self = Self(0x403);
    /// VMRUN refused the VMCB: an illegal guest state or control field.
    pub const INVALID: Self = Self(u64::MAX);
    /// VMRUN refused a VMSA that is busy (SEV-ES).
    pub const BUSY: Self = Self(u64::MAX - 1);

    /// The exception vector of an exception intercept.
    pub const fn exception(self) -> Option<u8> {
        match self.0 {
            0x040..=0x05F => Some((self.0 - 0x040) as u8),
            _ => None,
        }
    }

    /// The control register of a CR read or write intercept, and whether it
    /// was a write.
    pub const fn cr(self) -> Option<(u8, bool)> {
        match self.0 {
            0x000..=0x00F => Some((self.0 as u8, false)),
            0x010..=0x01F => Some(((self.0 - 0x010) as u8, true)),
            _ => None,
        }
    }

    /// The debug register of a DR read or write intercept, and whether it
    /// was a write.
    pub const fn dr(self) -> Option<(u8, bool)> {
        match self.0 {
            0x020..=0x02F => Some(((self.0 - 0x020) as u8, false)),
            0x030..=0x03F => Some(((self.0 - 0x030) as u8, true)),
            _ => None,
        }
    }

    /// The exit's name, for the kernel's log line: `npf`, `ioio`, `cr3-write`.
    pub fn name(self) -> &'static str {
        NAMES
            .iter()
            .find(|(code, _)| *code == self)
            .map(|(_, name)| *name)
            .unwrap_or(match self.0 {
                0x000..=0x00F => "cr-read",
                0x010..=0x01F => "cr-write",
                0x020..=0x02F => "dr-read",
                0x030..=0x03F => "dr-write",
                0x040..=0x05F => "exception",
                0x090..=0x09F => "cr-write-trap",
                _ => "unknown",
            })
    }
}

/// The names of the single exits, for [`ExitCode::name`].
const NAMES: &[(ExitCode, &str)] = &[
    (ExitCode::INTR, "intr"),
    (ExitCode::NMI, "nmi"),
    (ExitCode::SMI, "smi"),
    (ExitCode::INIT, "init"),
    (ExitCode::VINTR, "vintr"),
    (ExitCode::CR0_SEL_WRITE, "cr0-sel-write"),
    (ExitCode::RDTSC, "rdtsc"),
    (ExitCode::RDPMC, "rdpmc"),
    (ExitCode::CPUID, "cpuid"),
    (ExitCode::IRET, "iret"),
    (ExitCode::SWINT, "swint"),
    (ExitCode::INVD, "invd"),
    (ExitCode::PAUSE, "pause"),
    (ExitCode::HLT, "hlt"),
    (ExitCode::INVLPG, "invlpg"),
    (ExitCode::INVLPGA, "invlpga"),
    (ExitCode::IOIO, "ioio"),
    (ExitCode::MSR, "msr"),
    (ExitCode::TASK_SWITCH, "task-switch"),
    (ExitCode::SHUTDOWN, "shutdown"),
    (ExitCode::VMRUN, "vmrun"),
    (ExitCode::VMMCALL, "vmmcall"),
    (ExitCode::VMLOAD, "vmload"),
    (ExitCode::VMSAVE, "vmsave"),
    (ExitCode::STGI, "stgi"),
    (ExitCode::CLGI, "clgi"),
    (ExitCode::SKINIT, "skinit"),
    (ExitCode::RDTSCP, "rdtscp"),
    (ExitCode::ICEBP, "icebp"),
    (ExitCode::WBINVD, "wbinvd"),
    (ExitCode::MONITOR, "monitor"),
    (ExitCode::MWAIT, "mwait"),
    (ExitCode::MWAIT_CONDITIONAL, "mwait-conditional"),
    (ExitCode::XSETBV, "xsetbv"),
    (ExitCode::RDPRU, "rdpru"),
    (ExitCode::EFER_WRITE_TRAP, "efer-write-trap"),
    (ExitCode::INVPCID, "invpcid"),
    (ExitCode::BUS_LOCK, "bus-lock"),
    (ExitCode::IDLE_HLT, "idle-hlt"),
    (ExitCode::NPF, "npf"),
    (ExitCode::AVIC_INCOMPLETE_IPI, "avic-incomplete-ipi"),
    (ExitCode::AVIC_NOACCEL, "avic-noaccel"),
    (ExitCode::VMGEXIT, "vmgexit"),
    (ExitCode::INVALID, "invalid"),
    (ExitCode::BUSY, "busy"),
];

/// An IOIO intercept's EXITINFO1 (APM 15.10.2): the port, direction, size and
/// form of the access. EXITINFO2 holds the next instruction's RIP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ioio {
    pub port: u16,
    /// IN or INS; else OUT or OUTS.
    pub input: bool,
    /// INS or OUTS.
    pub string: bool,
    /// With a REP prefix.
    pub rep: bool,
    /// Operand size in bytes: 1, 2 or 4.
    pub size: u8,
    /// Address size in bits for a string access: 16, 32 or 64; 0 when the
    /// CPU set none.
    pub address_bits: u8,
    /// The segment of a string OUTS (0 ES ... 5 GS).
    pub segment: u8,
}

impl Ioio {
    /// Decodes EXITINFO1. None when no operand size bit, or more than one,
    /// is set: a VMCB the CPU did not write.
    pub const fn decode(info1: u64) -> Option<Self> {
        let size = match (info1 >> 4) & 0b111 {
            0b001 => 1,
            0b010 => 2,
            0b100 => 4,
            _ => return None,
        };
        let address_bits = match (info1 >> 7) & 0b111 {
            0b001 => 16,
            0b010 => 32,
            0b100 => 64,
            _ => 0,
        };
        Some(Self {
            port: (info1 >> 16) as u16,
            input: info1 & 1 != 0,
            string: info1 & (1 << 2) != 0,
            rep: info1 & (1 << 3) != 0,
            size,
            address_bits,
            segment: ((info1 >> 10) & 0b111) as u8,
        })
    }

    /// EXITINFO1 for this access, as the CPU writes it.
    pub const fn encode(self) -> u64 {
        let size = match self.size {
            1 => 0b001,
            2 => 0b010,
            _ => 0b100,
        };
        let address = match self.address_bits {
            16 => 0b001,
            32 => 0b010,
            64 => 0b100,
            _ => 0,
        };
        (self.port as u64) << 16
            | (self.segment as u64 & 0b111) << 10
            | address << 7
            | size << 4
            | (self.rep as u64) << 3
            | (self.string as u64) << 2
            | self.input as u64
    }
}

/// An MSR intercept's EXITINFO1: 0 for RDMSR, 1 for WRMSR (APM 15.11). The
/// MSR is the guest's ECX; a write's value its EDX:EAX.
pub const fn msr_write(info1: u64) -> bool {
    info1 & 1 != 0
}

/// A nested page fault (APM 15.25.6): EXITINFO1 is an error code like #PF's,
/// with two more bits; EXITINFO2 the faulting guest physical address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Npf {
    pub gpa: u64,
    /// The nested entry was present: a protection fault, not a missing page.
    pub present: bool,
    pub write: bool,
    pub user: bool,
    /// A reserved bit was set in a nested entry.
    pub reserved: bool,
    /// An instruction fetch.
    pub fetch: bool,
    /// The fault was on the guest's final physical address.
    pub final_address: bool,
    /// The fault was on a guest page table, during the guest's own walk.
    pub page_table: bool,
}

impl Npf {
    pub const fn decode(info1: u64, info2: u64) -> Self {
        Self {
            gpa: info2,
            present: info1 & (1 << 0) != 0,
            write: info1 & (1 << 1) != 0,
            user: info1 & (1 << 2) != 0,
            reserved: info1 & (1 << 3) != 0,
            fetch: info1 & (1 << 4) != 0,
            final_address: info1 & (1 << 32) != 0,
            page_table: info1 & (1 << 33) != 0,
        }
    }
}

// What the kernel does with an exit (P6 row 3). Everything below is pure:
// the run loop reads an [`Exit`] out of the VMCB, asks [`dispatch`], and acts.

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
    /// PAUSE in a spin loop: the vCPU yields. PAUSE runs again on resume,
    /// which costs nothing, so RIP stays.
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
        }
    }
}

/// The longest x86 instruction.
pub const MAX_INSN: usize = 15;

/// The length of `insn` in `bytes`, read at the guest's RIP: its prefixes
/// (legacy ones, and REX in 64-bit mode, `long`), then its opcode. None when
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
/// only while the kernel waits for an interrupt window.
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
    match code {
        ExitCode::INTR | ExitCode::NMI | ExitCode::SMI | ExitCode::INIT => {
            Action::Kernel(Kernel::HostEvent, Next::Stay)
        }
        ExitCode::VINTR => Action::Kernel(Kernel::InterruptWindow, Next::Stay),
        ExitCode::CPUID => kernel(Kernel::Cpuid, Insn::Cpuid),
        ExitCode::HLT | ExitCode::IDLE_HLT => kernel(Kernel::Hlt, Insn::Hlt),
        ExitCode::PAUSE => Action::Kernel(Kernel::Pause, Next::Stay),
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
/// too. Past the guest's size there is nothing to emulate.
fn npf(exit: &Exit, guest: &Guest) -> Action {
    let fault = Npf::decode(exit.info1, exit.info2);
    let gpa = fault.gpa;
    let rom_write = guest
        .ram
        .find(gpa)
        .is_some_and(|region| fault.write && !region.access.write);
    let in_ram = guest.ram.find(gpa).is_some() && !rom_write;
    if in_ram || fault.reserved || fault.page_table || fault.fetch || gpa >= guest.ram.guest() {
        return Action::Kill(Kill::Nested(fault));
    }
    let apic = |base: u64| gpa >= base && gpa - base < APIC_WINDOW;
    if apic(IOAPIC) || apic(LAPIC) {
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
