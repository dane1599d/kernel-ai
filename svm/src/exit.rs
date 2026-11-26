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
    (ExitCode::IDTR_READ, "idtr-read"),
    (ExitCode::GDTR_READ, "gdtr-read"),
    (ExitCode::LDTR_READ, "ldtr-read"),
    (ExitCode::TR_READ, "tr-read"),
    (ExitCode::IDTR_WRITE, "idtr-write"),
    (ExitCode::GDTR_WRITE, "gdtr-write"),
    (ExitCode::LDTR_WRITE, "ldtr-write"),
    (ExitCode::TR_WRITE, "tr-write"),
    (ExitCode::RDTSC, "rdtsc"),
    (ExitCode::RDPMC, "rdpmc"),
    (ExitCode::PUSHF, "pushf"),
    (ExitCode::POPF, "popf"),
    (ExitCode::CPUID, "cpuid"),
    (ExitCode::RSM, "rsm"),
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
    (ExitCode::FERR_FREEZE, "ferr-freeze"),
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

/// EXITINFO1's reserved bits for an IOIO intercept (APM 15.10.2, figure
/// 15-2: bits 1 and 15:13; the figure gives nothing above bit 31).
const RESERVED: u64 = 1 << 1 | 0b111 << 13;

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
    /// is set, or a reserved bit (1 or 15:13): a VMCB the CPU did not
    /// write.
    pub const fn decode(info1: u64) -> Option<Self> {
        if info1 & RESERVED != 0 {
            return None;
        }
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

mod dispatch;
mod msr;

pub use dispatch::*;
pub use msr::{MSR_ALLOWED, msr, msr_allowed};
