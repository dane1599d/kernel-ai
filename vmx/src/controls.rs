//! What exits to the hypervisor and what VM entry and exit load (SDM vol. 3C,
//! 25.6-25.8; the capability MSRs in vol. 3D, appendix A.3-A.5): the
//! pin-based, primary and secondary processor-based execution controls, the
//! exit and entry controls, and the exception bitmap, as a set.
//!
//! Unlike SVM's intercept vectors, VMX makes some instructions exit always
//! (CPUID, GETSEC, INVD, XSETBV and every VMX instruction, 26.1.2), so the
//! set holds only what the hypervisor may choose. A CPU fixes some control
//! bits to 1 or 0; [`Controls::adjust`] fits a set to its capability MSRs or
//! says which bits it cannot have, never dropping a bit the kernel asked for.

/// Pin-based execution controls (VMCS 0x4000; table 25-5).
pub mod pin {
    pub const EXTERNAL_INTERRUPT_EXITING: u32 = 1 << 0;
    pub const NMI_EXITING: u32 = 1 << 3;
    pub const VIRTUAL_NMIS: u32 = 1 << 5;
    pub const PREEMPTION_TIMER: u32 = 1 << 6;
    pub const POSTED_INTERRUPTS: u32 = 1 << 7;
}

/// Primary processor-based execution controls (VMCS 0x4002; table 25-6).
pub mod primary {
    pub const INTERRUPT_WINDOW_EXITING: u32 = 1 << 2;
    pub const TSC_OFFSETTING: u32 = 1 << 3;
    pub const HLT_EXITING: u32 = 1 << 7;
    pub const INVLPG_EXITING: u32 = 1 << 9;
    pub const MWAIT_EXITING: u32 = 1 << 10;
    pub const RDPMC_EXITING: u32 = 1 << 11;
    pub const RDTSC_EXITING: u32 = 1 << 12;
    pub const CR3_LOAD_EXITING: u32 = 1 << 15;
    pub const CR3_STORE_EXITING: u32 = 1 << 16;
    pub const TERTIARY_CONTROLS: u32 = 1 << 17;
    pub const CR8_LOAD_EXITING: u32 = 1 << 19;
    pub const CR8_STORE_EXITING: u32 = 1 << 20;
    pub const TPR_SHADOW: u32 = 1 << 21;
    pub const NMI_WINDOW_EXITING: u32 = 1 << 22;
    pub const MOV_DR_EXITING: u32 = 1 << 23;
    pub const UNCONDITIONAL_IO_EXITING: u32 = 1 << 24;
    pub const IO_BITMAPS: u32 = 1 << 25;
    pub const MONITOR_TRAP_FLAG: u32 = 1 << 27;
    pub const MSR_BITMAPS: u32 = 1 << 28;
    pub const MONITOR_EXITING: u32 = 1 << 29;
    pub const PAUSE_EXITING: u32 = 1 << 30;
    pub const SECONDARY_CONTROLS: u32 = 1 << 31;
}

/// Secondary processor-based execution controls (VMCS 0x401E; table 25-7).
pub mod secondary {
    pub const VIRTUALIZE_APIC_ACCESSES: u32 = 1 << 0;
    pub const EPT: u32 = 1 << 1;
    pub const DESCRIPTOR_TABLE_EXITING: u32 = 1 << 2;
    pub const RDTSCP: u32 = 1 << 3;
    pub const VIRTUALIZE_X2APIC: u32 = 1 << 4;
    pub const VPID: u32 = 1 << 5;
    pub const WBINVD_EXITING: u32 = 1 << 6;
    pub const UNRESTRICTED_GUEST: u32 = 1 << 7;
    pub const APIC_REGISTER_VIRTUALIZATION: u32 = 1 << 8;
    pub const VIRTUAL_INTERRUPT_DELIVERY: u32 = 1 << 9;
    pub const PAUSE_LOOP_EXITING: u32 = 1 << 10;
    pub const RDRAND_EXITING: u32 = 1 << 11;
    pub const INVPCID: u32 = 1 << 12;
    pub const VM_FUNCTIONS: u32 = 1 << 13;
    pub const VMCS_SHADOWING: u32 = 1 << 14;
    pub const ENCLS_EXITING: u32 = 1 << 15;
    pub const RDSEED_EXITING: u32 = 1 << 16;
    pub const PML: u32 = 1 << 17;
    pub const EPT_VIOLATION_VE: u32 = 1 << 18;
    pub const CONCEAL_VMX_FROM_PT: u32 = 1 << 19;
    pub const XSAVES: u32 = 1 << 20;
    pub const MODE_BASED_EPT_EXECUTE: u32 = 1 << 22;
    pub const TSC_SCALING: u32 = 1 << 25;
    pub const USER_WAIT_PAUSE: u32 = 1 << 26;
    pub const BUS_LOCK_DETECTION: u32 = 1 << 30;
    pub const INSTRUCTION_TIMEOUT: u32 = 1 << 31;
}

/// Primary VM-exit controls (VMCS 0x400C; table 25-13).
pub mod exit {
    pub const SAVE_DEBUG_CONTROLS: u32 = 1 << 2;
    /// The host is 64-bit: exits return to long mode.
    pub const HOST_ADDRESS_SPACE_SIZE: u32 = 1 << 9;
    pub const LOAD_PERF_GLOBAL_CTRL: u32 = 1 << 12;
    pub const ACKNOWLEDGE_INTERRUPT: u32 = 1 << 15;
    pub const SAVE_PAT: u32 = 1 << 18;
    pub const LOAD_PAT: u32 = 1 << 19;
    pub const SAVE_EFER: u32 = 1 << 20;
    pub const LOAD_EFER: u32 = 1 << 21;
    pub const SAVE_PREEMPTION_TIMER: u32 = 1 << 22;
    pub const CLEAR_BNDCFGS: u32 = 1 << 23;
    pub const LOAD_CET: u32 = 1 << 28;
}

/// VM-entry controls (VMCS 0x4012; table 25-15).
pub mod entry {
    pub const LOAD_DEBUG_CONTROLS: u32 = 1 << 2;
    /// The guest runs in IA-32e (long) mode after entry.
    pub const IA32E_MODE_GUEST: u32 = 1 << 9;
    pub const ENTRY_TO_SMM: u32 = 1 << 10;
    pub const DEACTIVATE_DUAL_MONITOR: u32 = 1 << 11;
    pub const LOAD_PERF_GLOBAL_CTRL: u32 = 1 << 13;
    pub const LOAD_PAT: u32 = 1 << 14;
    pub const LOAD_EFER: u32 = 1 << 15;
    pub const LOAD_BNDCFGS: u32 = 1 << 16;
    pub const LOAD_CET: u32 = 1 << 20;
}

/// Exception vectors, for the exception bitmap (VMCS 0x4004).
pub mod vector {
    pub const DEBUG: u8 = 1;
    pub const PAGE_FAULT: u8 = 14;
    pub const ALIGNMENT_CHECK: u8 = 17;
    pub const MACHINE_CHECK: u8 = 18;
}

/// Which of the five control words a bit is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Word {
    Pin,
    Primary,
    Secondary,
    Exit,
    Entry,
}

/// Why a set of controls cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The CPU cannot set these bits of the word (their allowed-1 bit is 0),
    /// or the capability MSR fixes them both to 1 and to 0.
    Unsupported(Word, u32),
    /// The set lacks bits of the host's minimum ([`Controls::required`]).
    BelowMinimum(Word, u32),
    /// The exception bitmap lacks these vectors of the minimum.
    ExceptionsBelowMinimum(u32),
}

/// One set of controls, as the VMCS holds them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Controls {
    pub pin: u32,
    pub primary: u32,
    pub secondary: u32,
    pub exit: u32,
    pub entry: u32,
    /// One bit an exception vector: a set bit makes the exception exit.
    pub exceptions: u32,
}

/// The capability MSRs for the five words: for each, bits 31:0 are the
/// allowed-0 settings (a set bit must be 1) and bits 63:32 the allowed-1
/// settings (a clear bit must be 0). The `TRUE_` MSRs when IA32_VMX_BASIC
/// bit 55 says they exist, as [`crate::vmcs::Basic`]`::true_controls` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub pin: u64,
    pub primary: u64,
    /// IA32_VMX_PROCBASED_CTLS2, whose allowed-0 half is always 0.
    pub secondary: u64,
    pub exit: u64,
    pub entry: u64,
}

impl Controls {
    pub const fn none() -> Self {
        Self {
            pin: 0,
            primary: 0,
            secondary: 0,
            exit: 0,
            entry: 0,
            exceptions: 0,
        }
    }

    /// The host's minimum: what every VMCS the kernel runs sets, so the
    /// guest cannot reach past its own state. Each is here because:
    ///
    /// - **External-interrupt and NMI exiting**: physical interrupts and
    ///   NMIs exit to the host, so its timer, IPIs and watchdog keep the CPU
    ///   while a guest runs. Without them the guest's IDT takes them.
    /// - **I/O bitmaps**: with neither these nor unconditional I/O exiting,
    ///   the guest's IN and OUT reach every port itself (26.1.3).
    /// - **Secondary controls and EPT**: without EPT, the guest's paging
    ///   maps host physical memory directly, and the guest owns the machine.
    /// - **Host address-space size**: exits return to a 64-bit host; without
    ///   it VM entry fails on a 64-bit host's state.
    /// - **Save and load EFER and PAT**, on entry and exit: the guest's EFER
    ///   and PAT never stay in the CPU after an exit (a guest clearing
    ///   EFER.NXE would turn off the host's no-execute).
    /// - **#DB, #AC and #MC in the exception bitmap**: a guest can make #DB
    ///   or #AC deliver forever without taking an interrupt, wedging the
    ///   host's CPU (CVE-2015-8104 and CVE-2015-5307, both found in KVM);
    ///   #MC is the host's to handle.
    pub const fn required() -> Self {
        Self {
            pin: pin::EXTERNAL_INTERRUPT_EXITING | pin::NMI_EXITING,
            primary: primary::IO_BITMAPS | primary::SECONDARY_CONTROLS,
            secondary: secondary::EPT,
            exit: exit::HOST_ADDRESS_SPACE_SIZE
                | exit::SAVE_EFER
                | exit::LOAD_EFER
                | exit::SAVE_PAT
                | exit::LOAD_PAT,
            entry: entry::LOAD_EFER | entry::LOAD_PAT,
            exceptions: 1 << vector::DEBUG
                | 1 << vector::ALIGNMENT_CHECK
                | 1 << vector::MACHINE_CHECK,
        }
    }

    /// The union of two sets.
    pub const fn with(self, other: Self) -> Self {
        Self {
            pin: self.pin | other.pin,
            primary: self.primary | other.primary,
            secondary: self.secondary | other.secondary,
            exit: self.exit | other.exit,
            entry: self.entry | other.entry,
            exceptions: self.exceptions | other.exceptions,
        }
    }

    /// The set with exception `vector` (0-31) exiting; a higher vector is
    /// not an exception and is ignored.
    pub const fn with_exception(mut self, vector: u8) -> Self {
        if vector < 32 {
            self.exceptions |= 1 << vector;
        }
        self
    }

    /// Whether the set holds the host's minimum: Err names the first word
    /// that lacks bits of it. The kernel checks every set before VMLAUNCH,
    /// including one a caller built field by field.
    pub const fn check(&self) -> Result<(), Refusal> {
        let need = Self::required();
        let words = [
            (Word::Pin, self.pin, need.pin),
            (Word::Primary, self.primary, need.primary),
            (Word::Secondary, self.secondary, need.secondary),
            (Word::Exit, self.exit, need.exit),
            (Word::Entry, self.entry, need.entry),
        ];
        let mut i = 0;
        while i < words.len() {
            let (word, have, need) = words[i];
            if have & need != need {
                return Err(Refusal::BelowMinimum(word, need & !have));
            }
            i += 1;
        }
        if self.exceptions & need.exceptions != need.exceptions {
            return Err(Refusal::ExceptionsBelowMinimum(
                need.exceptions & !self.exceptions,
            ));
        }
        Ok(())
    }

    /// The set as this CPU can run it: each word with the bits the CPU fixes
    /// to 1 added. Err when the set has a bit the CPU cannot set, naming the
    /// word and bits; nothing the kernel asked for is dropped.
    pub const fn adjust(self, caps: &Capabilities) -> Result<Self, Refusal> {
        let pin = match fit(Word::Pin, self.pin, caps.pin) {
            Ok(v) => v,
            Err(e) => return Err(e),
        };
        let primary = match fit(Word::Primary, self.primary, caps.primary) {
            Ok(v) => v,
            Err(e) => return Err(e),
        };
        let secondary = match fit(Word::Secondary, self.secondary, caps.secondary) {
            Ok(v) => v,
            Err(e) => return Err(e),
        };
        let exit = match fit(Word::Exit, self.exit, caps.exit) {
            Ok(v) => v,
            Err(e) => return Err(e),
        };
        let entry = match fit(Word::Entry, self.entry, caps.entry) {
            Ok(v) => v,
            Err(e) => return Err(e),
        };
        Ok(Self {
            pin,
            primary,
            secondary,
            exit,
            entry,
            exceptions: self.exceptions,
        })
    }
}

/// One word fitted to its capability MSR (A.3.1).
const fn fit(word: Word, want: u32, cap: u64) -> Result<u32, Refusal> {
    let must = cap as u32;
    let may = (cap >> 32) as u32;
    let value = want | must;
    // A bit the CPU fixes to 1 but does not allow to be 1 is a capability
    // MSR no CPU reports: refused with the rest, not trusted.
    let bad = value & !may;
    if bad != 0 {
        return Err(Refusal::Unsupported(word, bad));
    }
    Ok(value)
}
