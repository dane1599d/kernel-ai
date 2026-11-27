//! VMCS field encodings (SDM vol. 3C, 25.11.2, and vol. 3D, appendix B): the
//! 32-bit value VMREAD and VMWRITE take to name a field.
//!
//! - Bit 0: access type, 0 full, 1 the high 32 bits of a 64-bit field.
//! - Bits 9:1: the index.
//! - Bits 11:10: the type: control, VM-exit information (read-only data),
//!   guest state or host state.
//! - Bit 12: reserved, 0.
//! - Bits 14:13: the width: 16-bit, 64-bit, 32-bit or natural.
//! - Bits 31:15: reserved, 0.

/// A field's width (bits 14:13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Width {
    Bits16 = 0,
    Bits64 = 1,
    Bits32 = 2,
    /// 64 bits on a CPU with Intel 64 architecture, as the kernel's.
    Natural = 3,
}

/// A field's type (bits 11:10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Control = 0,
    /// VM-exit information: read-only unless IA32_VMX_MISC bit 29 says
    /// VMWRITE may write it.
    ExitInfo = 1,
    Guest = 2,
    Host = 3,
}

/// Why a 32-bit value is not a field encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadField {
    /// Bit 12 or bits 31:15 are set.
    Reserved(u32),
    /// High access (bit 0) to a field that is not 64-bit.
    HighOfNarrow(u32),
}

const HIGH: u32 = 1;
const INDEX_SHIFT: u32 = 1;
const KIND_SHIFT: u32 = 10;
const WIDTH_SHIFT: u32 = 13;
/// Bit 12 and bits 31:15.
const RESERVED: u32 = 1 << 12 | !0x7FFF;

/// One VMCS field encoding, valid by construction or by [`Field::decode`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Field(u32);

impl Field {
    /// The full field of `width` and `kind` at `index` (bits 9:1, so 0-511;
    /// higher bits of `index` are dropped).
    pub const fn new(width: Width, kind: Kind, index: u16) -> Self {
        Self(
            (width as u32) << WIDTH_SHIFT
                | (kind as u32) << KIND_SHIFT
                | ((index as u32) & 0x1FF) << INDEX_SHIFT,
        )
    }

    /// Reads an encoding the kernel did not make: one taken from a guest's
    /// VMREAD or VMWRITE operand (nested VMX), or a table. Refuses reserved
    /// bits and high access to a field narrower than 64 bits, as VMREAD
    /// would (VM-instruction error 12).
    pub const fn decode(raw: u32) -> Result<Self, BadField> {
        if raw & RESERVED != 0 {
            return Err(BadField::Reserved(raw));
        }
        let field = Self(raw);
        if raw & HIGH != 0 && !matches!(field.width(), Width::Bits64) {
            return Err(BadField::HighOfNarrow(raw));
        }
        Ok(field)
    }

    /// The encoding, for VMREAD and VMWRITE.
    pub const fn raw(self) -> u32 {
        self.0
    }

    pub const fn width(self) -> Width {
        match (self.0 >> WIDTH_SHIFT) & 0b11 {
            0 => Width::Bits16,
            1 => Width::Bits64,
            2 => Width::Bits32,
            _ => Width::Natural,
        }
    }

    pub const fn kind(self) -> Kind {
        match (self.0 >> KIND_SHIFT) & 0b11 {
            0 => Kind::Control,
            1 => Kind::ExitInfo,
            2 => Kind::Guest,
            _ => Kind::Host,
        }
    }

    pub const fn index(self) -> u16 {
        ((self.0 >> INDEX_SHIFT) & 0x1FF) as u16
    }

    /// Whether this is the high half of a 64-bit field.
    pub const fn is_high(self) -> bool {
        self.0 & HIGH != 0
    }

    /// The high 32 bits of a 64-bit field, for a 32-bit VMREAD. None for a
    /// narrower field, or one already high.
    pub const fn high(self) -> Option<Self> {
        match (self.width(), self.is_high()) {
            (Width::Bits64, false) => Some(Self(self.0 | HIGH)),
            _ => None,
        }
    }

    /// The field's size in bytes, as VMREAD returns it in 64-bit mode (a
    /// high access reads 4).
    pub const fn bytes(self) -> u8 {
        match (self.width(), self.is_high()) {
            (Width::Bits16, _) => 2,
            (Width::Bits32, _) | (Width::Bits64, true) => 4,
            (Width::Bits64, false) | (Width::Natural, _) => 8,
        }
    }

    /// Whether VMWRITE refuses it without IA32_VMX_MISC bit 29.
    pub const fn read_only(self) -> bool {
        matches!(self.kind(), Kind::ExitInfo)
    }
}

macro_rules! fields {
    ($($name:ident = $raw:expr;)*) => {
        impl Field {
            $(pub const $name: Self = Self($raw);)*
        }
        /// Every named field and its name, for tests and the kernel's log.
        pub const ALL: &[(Field, &str)] = &[$((Field::$name, stringify!($name)),)*];
    };
}

fields! {
    // 16-bit control (B.1.1).
    VPID = 0x0000;
    POSTED_INTERRUPT_VECTOR = 0x0002;
    EPTP_INDEX = 0x0004;
    // 16-bit guest state (B.1.2).
    GUEST_ES_SELECTOR = 0x0800;
    GUEST_CS_SELECTOR = 0x0802;
    GUEST_SS_SELECTOR = 0x0804;
    GUEST_DS_SELECTOR = 0x0806;
    GUEST_FS_SELECTOR = 0x0808;
    GUEST_GS_SELECTOR = 0x080A;
    GUEST_LDTR_SELECTOR = 0x080C;
    GUEST_TR_SELECTOR = 0x080E;
    GUEST_INTERRUPT_STATUS = 0x0810;
    GUEST_PML_INDEX = 0x0812;
    // 16-bit host state (B.1.3).
    HOST_ES_SELECTOR = 0x0C00;
    HOST_CS_SELECTOR = 0x0C02;
    HOST_SS_SELECTOR = 0x0C04;
    HOST_DS_SELECTOR = 0x0C06;
    HOST_FS_SELECTOR = 0x0C08;
    HOST_GS_SELECTOR = 0x0C0A;
    HOST_TR_SELECTOR = 0x0C0C;
    // 64-bit control (B.2.1).
    IO_BITMAP_A = 0x2000;
    IO_BITMAP_B = 0x2002;
    MSR_BITMAP = 0x2004;
    EXIT_MSR_STORE_ADDRESS = 0x2006;
    EXIT_MSR_LOAD_ADDRESS = 0x2008;
    ENTRY_MSR_LOAD_ADDRESS = 0x200A;
    EXECUTIVE_VMCS_POINTER = 0x200C;
    PML_ADDRESS = 0x200E;
    TSC_OFFSET = 0x2010;
    VIRTUAL_APIC_ADDRESS = 0x2012;
    APIC_ACCESS_ADDRESS = 0x2014;
    POSTED_INTERRUPT_DESCRIPTOR = 0x2016;
    VM_FUNCTION_CONTROLS = 0x2018;
    EPT_POINTER = 0x201A;
    EOI_EXIT_BITMAP_0 = 0x201C;
    EOI_EXIT_BITMAP_1 = 0x201E;
    EOI_EXIT_BITMAP_2 = 0x2020;
    EOI_EXIT_BITMAP_3 = 0x2022;
    EPTP_LIST_ADDRESS = 0x2024;
    VMREAD_BITMAP = 0x2026;
    VMWRITE_BITMAP = 0x2028;
    VE_INFORMATION_ADDRESS = 0x202A;
    XSS_EXITING_BITMAP = 0x202C;
    ENCLS_EXITING_BITMAP = 0x202E;
    TSC_MULTIPLIER = 0x2032;
    TERTIARY_PROCBASED_CONTROLS = 0x2034;
    // 64-bit read-only data (B.2.2).
    GUEST_PHYSICAL_ADDRESS = 0x2400;
    // 64-bit guest state (B.2.3).
    VMCS_LINK_POINTER = 0x2800;
    GUEST_IA32_DEBUGCTL = 0x2802;
    GUEST_IA32_PAT = 0x2804;
    GUEST_IA32_EFER = 0x2806;
    GUEST_IA32_PERF_GLOBAL_CTRL = 0x2808;
    GUEST_PDPTE0 = 0x280A;
    GUEST_PDPTE1 = 0x280C;
    GUEST_PDPTE2 = 0x280E;
    GUEST_PDPTE3 = 0x2810;
    GUEST_IA32_BNDCFGS = 0x2812;
    GUEST_IA32_RTIT_CTL = 0x2814;
    // 64-bit host state (B.2.4).
    HOST_IA32_PAT = 0x2C00;
    HOST_IA32_EFER = 0x2C02;
    HOST_IA32_PERF_GLOBAL_CTRL = 0x2C04;
    // 32-bit control (B.3.1).
    PINBASED_CONTROLS = 0x4000;
    PROCBASED_CONTROLS = 0x4002;
    EXCEPTION_BITMAP = 0x4004;
    PAGE_FAULT_ERROR_CODE_MASK = 0x4006;
    PAGE_FAULT_ERROR_CODE_MATCH = 0x4008;
    CR3_TARGET_COUNT = 0x400A;
    EXIT_CONTROLS = 0x400C;
    EXIT_MSR_STORE_COUNT = 0x400E;
    EXIT_MSR_LOAD_COUNT = 0x4010;
    ENTRY_CONTROLS = 0x4012;
    ENTRY_MSR_LOAD_COUNT = 0x4014;
    ENTRY_INTERRUPTION_INFO = 0x4016;
    ENTRY_EXCEPTION_ERROR_CODE = 0x4018;
    ENTRY_INSTRUCTION_LENGTH = 0x401A;
    TPR_THRESHOLD = 0x401C;
    SECONDARY_PROCBASED_CONTROLS = 0x401E;
    PLE_GAP = 0x4020;
    PLE_WINDOW = 0x4022;
    // 32-bit read-only data (B.3.2).
    VM_INSTRUCTION_ERROR = 0x4400;
    EXIT_REASON = 0x4402;
    EXIT_INTERRUPTION_INFO = 0x4404;
    EXIT_INTERRUPTION_ERROR_CODE = 0x4406;
    IDT_VECTORING_INFO = 0x4408;
    IDT_VECTORING_ERROR_CODE = 0x440A;
    EXIT_INSTRUCTION_LENGTH = 0x440C;
    EXIT_INSTRUCTION_INFO = 0x440E;
    // 32-bit guest state (B.3.3).
    GUEST_ES_LIMIT = 0x4800;
    GUEST_CS_LIMIT = 0x4802;
    GUEST_SS_LIMIT = 0x4804;
    GUEST_DS_LIMIT = 0x4806;
    GUEST_FS_LIMIT = 0x4808;
    GUEST_GS_LIMIT = 0x480A;
    GUEST_LDTR_LIMIT = 0x480C;
    GUEST_TR_LIMIT = 0x480E;
    GUEST_GDTR_LIMIT = 0x4810;
    GUEST_IDTR_LIMIT = 0x4812;
    GUEST_ES_ACCESS_RIGHTS = 0x4814;
    GUEST_CS_ACCESS_RIGHTS = 0x4816;
    GUEST_SS_ACCESS_RIGHTS = 0x4818;
    GUEST_DS_ACCESS_RIGHTS = 0x481A;
    GUEST_FS_ACCESS_RIGHTS = 0x481C;
    GUEST_GS_ACCESS_RIGHTS = 0x481E;
    GUEST_LDTR_ACCESS_RIGHTS = 0x4820;
    GUEST_TR_ACCESS_RIGHTS = 0x4822;
    GUEST_INTERRUPTIBILITY = 0x4824;
    GUEST_ACTIVITY_STATE = 0x4826;
    GUEST_SMBASE = 0x4828;
    GUEST_IA32_SYSENTER_CS = 0x482A;
    PREEMPTION_TIMER_VALUE = 0x482E;
    // 32-bit host state (B.3.4).
    HOST_IA32_SYSENTER_CS = 0x4C00;
    // Natural-width control (B.4.1).
    CR0_GUEST_HOST_MASK = 0x6000;
    CR4_GUEST_HOST_MASK = 0x6002;
    CR0_READ_SHADOW = 0x6004;
    CR4_READ_SHADOW = 0x6006;
    CR3_TARGET_0 = 0x6008;
    CR3_TARGET_1 = 0x600A;
    CR3_TARGET_2 = 0x600C;
    CR3_TARGET_3 = 0x600E;
    // Natural-width read-only data (B.4.2).
    EXIT_QUALIFICATION = 0x6400;
    IO_RCX = 0x6402;
    IO_RSI = 0x6404;
    IO_RDI = 0x6406;
    IO_RIP = 0x6408;
    GUEST_LINEAR_ADDRESS = 0x640A;
    // Natural-width guest state (B.4.3).
    GUEST_CR0 = 0x6800;
    GUEST_CR3 = 0x6802;
    GUEST_CR4 = 0x6804;
    GUEST_ES_BASE = 0x6806;
    GUEST_CS_BASE = 0x6808;
    GUEST_SS_BASE = 0x680A;
    GUEST_DS_BASE = 0x680C;
    GUEST_FS_BASE = 0x680E;
    GUEST_GS_BASE = 0x6810;
    GUEST_LDTR_BASE = 0x6812;
    GUEST_TR_BASE = 0x6814;
    GUEST_GDTR_BASE = 0x6816;
    GUEST_IDTR_BASE = 0x6818;
    GUEST_DR7 = 0x681A;
    GUEST_RSP = 0x681C;
    GUEST_RIP = 0x681E;
    GUEST_RFLAGS = 0x6820;
    GUEST_PENDING_DEBUG_EXCEPTIONS = 0x6822;
    GUEST_IA32_SYSENTER_ESP = 0x6824;
    GUEST_IA32_SYSENTER_EIP = 0x6826;
    GUEST_IA32_S_CET = 0x6828;
    GUEST_SSP = 0x682A;
    GUEST_IA32_INTERRUPT_SSP_TABLE = 0x682C;
    // Natural-width host state (B.4.4).
    HOST_CR0 = 0x6C00;
    HOST_CR3 = 0x6C02;
    HOST_CR4 = 0x6C04;
    HOST_FS_BASE = 0x6C06;
    HOST_GS_BASE = 0x6C08;
    HOST_TR_BASE = 0x6C0A;
    HOST_GDTR_BASE = 0x6C0C;
    HOST_IDTR_BASE = 0x6C0E;
    HOST_IA32_SYSENTER_ESP = 0x6C10;
    HOST_IA32_SYSENTER_EIP = 0x6C12;
    HOST_RSP = 0x6C14;
    HOST_RIP = 0x6C16;
    HOST_IA32_S_CET = 0x6C18;
    HOST_SSP = 0x6C1A;
    HOST_IA32_INTERRUPT_SSP_TABLE = 0x6C1C;
}
