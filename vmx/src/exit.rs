//! The exit-reason field (VMCS 0x4402; SDM vol. 3C, 28.2.1, and vol. 3D,
//! appendix C) and the exit qualifications (0x6400) of the exits the kernel
//! handles itself (vol. 3C, 28.2.1, tables 28-1 to 28-7).
//!
//! The exit-reason field: bits 15:0 the basic exit reason, 27 the exit was
//! from enclave mode, 28 a pending MTF exit, 29 the exit was from VMX root
//! operation (SMM's dual monitor), 31 the VM entry failed. Bits 26:16 and 30
//! are 0.

/// Why the guest exited: bits 15:0 of the exit-reason field. Each value the
/// SDM defines; [`BasicExit::from_raw`] refuses the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum BasicExit {
    /// An exception or NMI; the exit interruption information says which.
    ExceptionOrNmi = 0,
    ExternalInterrupt = 1,
    /// A triple fault: the guest cannot go on.
    TripleFault = 2,
    InitSignal = 3,
    StartupIpi = 4,
    IoSmi = 5,
    OtherSmi = 6,
    InterruptWindow = 7,
    NmiWindow = 8,
    TaskSwitch = 9,
    Cpuid = 10,
    Getsec = 11,
    Hlt = 12,
    Invd = 13,
    Invlpg = 14,
    Rdpmc = 15,
    Rdtsc = 16,
    Rsm = 17,
    Vmcall = 18,
    Vmclear = 19,
    Vmlaunch = 20,
    Vmptrld = 21,
    Vmptrst = 22,
    Vmread = 23,
    Vmresume = 24,
    Vmwrite = 25,
    Vmxoff = 26,
    Vmxon = 27,
    /// MOV to or from CR, CLTS or LMSW ([`CrAccess`]).
    CrAccess = 28,
    /// MOV to or from DR ([`DrAccess`]).
    DrAccess = 29,
    /// IN, OUT, INS or OUTS ([`IoAccess`]).
    Io = 30,
    Rdmsr = 31,
    Wrmsr = 32,
    /// VM entry failed on the guest state ([`EntryFailure`]).
    EntryInvalidGuestState = 33,
    /// VM entry failed loading an MSR; the qualification is the entry's
    /// index, from 1.
    EntryMsrLoading = 34,
    Mwait = 36,
    MonitorTrapFlag = 37,
    Monitor = 39,
    Pause = 40,
    EntryMachineCheck = 41,
    TprBelowThreshold = 43,
    ApicAccess = 44,
    VirtualizedEoi = 45,
    GdtrIdtrAccess = 46,
    LdtrTrAccess = 47,
    /// An access the EPT entries do not allow ([`EptViolation`]).
    EptViolation = 48,
    /// An EPT entry that is not a valid encoding: a hypervisor bug, as the
    /// kernel's tables are its own.
    EptMisconfiguration = 49,
    Invept = 50,
    Rdtscp = 51,
    PreemptionTimer = 52,
    Invvpid = 53,
    /// WBINVD or WBNOINVD.
    Wbinvd = 54,
    Xsetbv = 55,
    ApicWrite = 56,
    Rdrand = 57,
    Invpcid = 58,
    Vmfunc = 59,
    Encls = 60,
    Rdseed = 61,
    PmlFull = 62,
    Xsaves = 63,
    Xrstors = 64,
    Pconfig = 65,
    SppEvent = 66,
    Umwait = 67,
    Tpause = 68,
    Loadiwkey = 69,
    Enclv = 70,
    EnqcmdPasidFailure = 72,
    EnqcmdsPasidFailure = 73,
    BusLock = 74,
    /// The notify window ran out: the guest held the CPU in one instruction.
    InstructionTimeout = 75,
    Seamcall = 76,
    Tdcall = 77,
    /// RDMSR with an immediate operand.
    RdmsrImmediate = 84,
    /// WRMSRNS with an immediate operand.
    WrmsrImmediate = 85,
}

/// Every basic exit, in order, for [`BasicExit::from_raw`] and the tests.
pub const ALL: &[BasicExit] = {
    use BasicExit::*;
    &[
        ExceptionOrNmi,
        ExternalInterrupt,
        TripleFault,
        InitSignal,
        StartupIpi,
        IoSmi,
        OtherSmi,
        InterruptWindow,
        NmiWindow,
        TaskSwitch,
        Cpuid,
        Getsec,
        Hlt,
        Invd,
        Invlpg,
        Rdpmc,
        Rdtsc,
        Rsm,
        Vmcall,
        Vmclear,
        Vmlaunch,
        Vmptrld,
        Vmptrst,
        Vmread,
        Vmresume,
        Vmwrite,
        Vmxoff,
        Vmxon,
        CrAccess,
        DrAccess,
        Io,
        Rdmsr,
        Wrmsr,
        EntryInvalidGuestState,
        EntryMsrLoading,
        Mwait,
        MonitorTrapFlag,
        Monitor,
        Pause,
        EntryMachineCheck,
        TprBelowThreshold,
        ApicAccess,
        VirtualizedEoi,
        GdtrIdtrAccess,
        LdtrTrAccess,
        EptViolation,
        EptMisconfiguration,
        Invept,
        Rdtscp,
        PreemptionTimer,
        Invvpid,
        Wbinvd,
        Xsetbv,
        ApicWrite,
        Rdrand,
        Invpcid,
        Vmfunc,
        Encls,
        Rdseed,
        PmlFull,
        Xsaves,
        Xrstors,
        Pconfig,
        SppEvent,
        Umwait,
        Tpause,
        Loadiwkey,
        Enclv,
        EnqcmdPasidFailure,
        EnqcmdsPasidFailure,
        BusLock,
        InstructionTimeout,
        Seamcall,
        Tdcall,
        RdmsrImmediate,
        WrmsrImmediate,
    ]
};

impl BasicExit {
    /// The basic exit `raw` names. Err for a number this table does not
    /// name: one the SDM leaves unused (35, 38, 42), or one past it, from a
    /// feature the kernel does not enable or a CPU newer than the table. The
    /// kernel stops the guest on it rather than guess.
    pub const fn from_raw(raw: u16) -> Result<Self, BadExit> {
        let mut i = 0;
        while i < ALL.len() {
            if ALL[i] as u16 == raw {
                return Ok(ALL[i]);
            }
            i += 1;
        }
        Err(BadExit::Unknown(raw))
    }

    /// Whether only a failed VM entry reports it (bit 31 then set).
    pub const fn entry_failure(self) -> bool {
        matches!(
            self,
            Self::EntryInvalidGuestState | Self::EntryMsrLoading | Self::EntryMachineCheck
        )
    }
}

/// Why an exit-reason field, or a qualification, cannot be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadExit {
    /// A basic exit reason the SDM does not define.
    Unknown(u16),
    /// Bits 26:16 or 30 of the exit-reason field are set.
    Reserved(u32),
    /// Bit 31 (entry failed) with a reason only a completed entry reports,
    /// or the other way round.
    FailureMismatch(u32),
    /// An access type, size or register number a qualification's format
    /// does not have.
    Qualification(u64),
}

const ENCLAVE: u32 = 1 << 27;
const PENDING_MTF: u32 = 1 << 28;
const FROM_ROOT: u32 = 1 << 29;
const ENTRY_FAILED: u32 = 1 << 31;
const EXIT_RESERVED: u32 = 0x07FF_0000 | 1 << 30;

/// The exit-reason field, decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExitReason {
    pub basic: BasicExit,
    pub enclave: bool,
    pub pending_mtf: bool,
    pub from_root: bool,
    pub entry_failed: bool,
}

impl ExitReason {
    pub const fn decode(raw: u32) -> Result<Self, BadExit> {
        if raw & EXIT_RESERVED != 0 {
            return Err(BadExit::Reserved(raw));
        }
        let basic = match BasicExit::from_raw(raw as u16) {
            Ok(basic) => basic,
            Err(e) => return Err(e),
        };
        let entry_failed = raw & ENTRY_FAILED != 0;
        // Exits 33, 34 and 41 come only from a failed entry, with bit 31,
        // and bit 31 comes with nothing else (28.8).
        if entry_failed != basic.entry_failure() {
            return Err(BadExit::FailureMismatch(raw));
        }
        Ok(Self {
            basic,
            enclave: raw & ENCLAVE != 0,
            pending_mtf: raw & PENDING_MTF != 0,
            from_root: raw & FROM_ROOT != 0,
            entry_failed,
        })
    }

    /// The field as the CPU writes it.
    pub const fn encode(self) -> u32 {
        self.basic as u32
            | ((self.enclave as u32) * ENCLAVE)
            | ((self.pending_mtf as u32) * PENDING_MTF)
            | ((self.from_root as u32) * FROM_ROOT)
            | ((self.entry_failed as u32) * ENTRY_FAILED)
    }
}

/// A control-register access's kind (qualification bits 5:4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrKind {
    /// MOV to CR`n` from general register `register`.
    MovTo {
        register: u8,
    },
    /// MOV from CR`n` to general register `register`.
    MovFrom {
        register: u8,
    },
    Clts,
    /// LMSW of `source` (the low 16 bits of CR0); `memory` when the operand
    /// was in memory, its linear address then in the guest-linear-address
    /// field.
    Lmsw {
        source: u16,
        memory: bool,
    },
}

/// A control-register access exit's qualification (table 28-3): bits 3:0 the
/// CR, 5:4 the kind, 6 LMSW's operand type, 11:8 the general register, 31:16
/// LMSW's source data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrAccess {
    /// The control register: 0, 3, 4 or 8 for MOV; 0 for CLTS and LMSW.
    pub cr: u8,
    pub kind: CrKind,
}

impl CrAccess {
    pub const fn decode(qualification: u64) -> Result<Self, BadExit> {
        let q = qualification;
        // Bits 7, 15:12 and 63:32 are 0.
        if q & !0xFFFF_0F7F != 0 {
            return Err(BadExit::Qualification(q));
        }
        let cr = (q & 0xF) as u8;
        let register = ((q >> 8) & 0xF) as u8;
        let kind = match (q >> 4) & 0b11 {
            0 => CrKind::MovTo { register },
            1 => CrKind::MovFrom { register },
            2 => CrKind::Clts,
            _ => CrKind::Lmsw {
                source: (q >> 16) as u16,
                memory: q & (1 << 6) != 0,
            },
        };
        // Only CR0, CR3, CR4 and CR8 can exit, and CLTS and LMSW are CR0's.
        let known = match kind {
            CrKind::MovTo { .. } => matches!(cr, 0 | 3 | 4 | 8),
            CrKind::MovFrom { .. } => matches!(cr, 3 | 8),
            CrKind::Clts | CrKind::Lmsw { .. } => cr == 0,
        };
        if !known {
            return Err(BadExit::Qualification(q));
        }
        Ok(Self { cr, kind })
    }
}

/// A debug-register access exit's qualification (table 28-4): bits 2:0 the
/// DR, bit 4 the direction (1 from DR), 11:8 the general register.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrAccess {
    pub dr: u8,
    /// MOV from DR (a read); else MOV to DR.
    pub read: bool,
    pub register: u8,
}

impl DrAccess {
    pub const fn decode(qualification: u64) -> Result<Self, BadExit> {
        let q = qualification;
        if q & !0x0F17 != 0 {
            return Err(BadExit::Qualification(q));
        }
        Ok(Self {
            dr: (q & 0b111) as u8,
            read: q & (1 << 4) != 0,
            register: ((q >> 8) & 0xF) as u8,
        })
    }
}

/// An I/O instruction exit's qualification (table 28-5): bits 2:0 the size
/// less 1 (0, 1 or 3), 3 the direction (1 IN), 4 a string instruction, 5 REP,
/// 6 the port was an immediate, 31:16 the port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoAccess {
    pub port: u16,
    /// Bytes: 1, 2 or 4.
    pub size: u8,
    /// IN or INS; else OUT or OUTS.
    pub input: bool,
    pub string: bool,
    pub rep: bool,
    /// The port was an immediate (`in al, 0x60`), not DX.
    pub immediate: bool,
}

impl IoAccess {
    pub const fn decode(qualification: u64) -> Result<Self, BadExit> {
        let q = qualification;
        if q & !0xFFFF_007F != 0 {
            return Err(BadExit::Qualification(q));
        }
        let size = match q & 0b111 {
            0 => 1,
            1 => 2,
            3 => 4,
            _ => return Err(BadExit::Qualification(q)),
        };
        Ok(Self {
            port: (q >> 16) as u16,
            size,
            input: q & (1 << 3) != 0,
            string: q & (1 << 4) != 0,
            rep: q & (1 << 5) != 0,
            immediate: q & (1 << 6) != 0,
        })
    }

    /// The qualification as the CPU writes it.
    pub const fn encode(self) -> u64 {
        let size = match self.size {
            1 => 0,
            2 => 1,
            _ => 3,
        };
        (self.port as u64) << 16
            | (self.immediate as u64) << 6
            | (self.rep as u64) << 5
            | (self.string as u64) << 4
            | (self.input as u64) << 3
            | size
    }
}

/// An EPT violation's qualification (table 28-7), its guest physical address
/// in VMCS 0x2400. Bits 2:0 the access, 6:3 what the entries allowed, 7 the
/// guest-linear-address field is valid, 8 the access was to the translated
/// address (else to a guest paging entry), 12 NMI unblocking by IRET.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EptViolation {
    pub gpa: u64,
    pub read: bool,
    pub write: bool,
    pub fetch: bool,
    /// The entries allowed a read, a write, a supervisor fetch, a user fetch
    /// (the last with mode-based execute control).
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
    pub user_executable: bool,
    /// The guest linear address field holds the address the guest used.
    pub linear_valid: bool,
    /// The access was to the final translated address, not to one of the
    /// guest's own page-table entries (meaningful with `linear_valid`).
    pub final_address: bool,
    /// The exit came while IRET was unblocking NMIs: the kernel blocks them
    /// again before it resumes.
    pub nmi_unblocking: bool,
}

impl EptViolation {
    /// Decodes the qualification and the address. Bits this crate does not
    /// name (9-11, 13 and up: paging-entry details, shadow stacks, TDX) are
    /// ignored, not refused: they only add detail.
    pub const fn decode(qualification: u64, gpa: u64) -> Self {
        let q = qualification;
        Self {
            gpa,
            read: q & (1 << 0) != 0,
            write: q & (1 << 1) != 0,
            fetch: q & (1 << 2) != 0,
            readable: q & (1 << 3) != 0,
            writable: q & (1 << 4) != 0,
            executable: q & (1 << 5) != 0,
            user_executable: q & (1 << 6) != 0,
            linear_valid: q & (1 << 7) != 0,
            final_address: q & (1 << 8) != 0,
            nmi_unblocking: q & (1 << 12) != 0,
        }
    }

    /// Whether the page was not mapped at all: no right in any entry.
    pub const fn not_present(&self) -> bool {
        !(self.readable || self.writable || self.executable || self.user_executable)
    }
}

/// An entry failure's qualification (exit 33, table 28-1's last rows): what
/// part of the guest state failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryFailure {
    /// A guest-state check other than these.
    Default,
    /// A PDPTE failed to load (PAE paging).
    Pdpte,
    /// NMI injection was blocked by the guest's interruptibility state.
    NmiInjection,
    /// The VMCS link pointer is not valid.
    VmcsLinkPointer,
}

impl EntryFailure {
    pub const fn decode(qualification: u64) -> Result<Self, BadExit> {
        match qualification {
            0 => Ok(Self::Default),
            2 => Ok(Self::Pdpte),
            3 => Ok(Self::NmiInjection),
            4 => Ok(Self::VmcsLinkPointer),
            q => Err(BadExit::Qualification(q)),
        }
    }
}
