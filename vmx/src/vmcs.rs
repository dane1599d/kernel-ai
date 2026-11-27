//! IA32_VMX_BASIC (SDM vol. 3D, A.1), the VMCS and VMXON regions' header
//! (vol. 3C, 25.2 and 24.11.5), and the VM-instruction error numbers (vol.
//! 3C, 31.4) a failed VMX instruction leaves in VMCS 0x4400.

/// IA32_VMX_BASIC, decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Basic {
    /// The VMCS revision identifier (bits 30:0), which every VMCS and VMXON
    /// region starts with.
    pub revision: u32,
    /// Bytes the CPU uses of a region (bits 44:32): at most 4096.
    pub region_size: u16,
    /// The regions' addresses are limited to 32 bits (bit 48).
    pub addresses_32bit: bool,
    /// The memory type the CPU accesses the VMCS with (bits 53:50): write-back.
    pub write_back: bool,
    /// Exits for INS and OUTS give the instruction information (bit 54).
    pub ins_outs_info: bool,
    /// The TRUE_ capability MSRs exist (bit 55), and
    /// [`crate::controls::Controls::adjust`] is given them instead.
    pub true_controls: bool,
}

/// Why IA32_VMX_BASIC, or an instruction error, cannot be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadBasic {
    /// Bit 31 is set; the SDM says it is always 0.
    Bit31(u64),
    /// A region size of 0 or more than 4096 bytes.
    RegionSize(u16),
    /// A memory type other than UC (0) or WB (6).
    MemoryType(u8),
}

impl Basic {
    pub const fn decode(raw: u64) -> Result<Self, BadBasic> {
        if raw & (1 << 31) != 0 {
            return Err(BadBasic::Bit31(raw));
        }
        let region_size = ((raw >> 32) & 0x1FFF) as u16;
        if region_size == 0 || region_size > 4096 {
            return Err(BadBasic::RegionSize(region_size));
        }
        let write_back = match (raw >> 50) & 0xF {
            0 => false,
            6 => true,
            t => return Err(BadBasic::MemoryType(t as u8)),
        };
        Ok(Self {
            revision: (raw & 0x7FFF_FFFF) as u32,
            region_size,
            addresses_32bit: raw & (1 << 48) != 0,
            write_back,
            ins_outs_info: raw & (1 << 54) != 0,
            true_controls: raw & (1 << 55) != 0,
        })
    }
}

/// A VMCS or VMXON region: a 4 KiB page that starts with the revision
/// identifier (bit 31 set for a shadow VMCS) and the VMX-abort indicator.
/// The rest is the CPU's, in a format the SDM does not give: it is read and
/// written only by VMREAD and VMWRITE.
#[derive(Clone, Debug, PartialEq, Eq)]
#[repr(C, align(4096))]
pub struct Region {
    pub revision: u32,
    /// Written by the CPU on a VMX abort; the region's owner reads it.
    pub abort: u32,
    pub data: [u8; 4088],
}

const SHADOW: u32 = 1 << 31;

impl Region {
    /// An ordinary VMCS, or a VMXON region, for `basic`'s revision.
    pub const fn new(basic: &Basic) -> Self {
        Self {
            revision: basic.revision & !SHADOW,
            abort: 0,
            data: [0; 4088],
        }
    }

    /// Whether the region is marked as a shadow VMCS.
    pub const fn shadow(&self) -> bool {
        self.revision & SHADOW != 0
    }
}

/// Why a VMX instruction failed with a VMCS loaded (VM-instruction error).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum InstructionError {
    VmcallInRoot = 1,
    VmclearInvalidAddress = 2,
    VmclearVmxonPointer = 3,
    VmlaunchNonClear = 4,
    VmresumeNonLaunched = 5,
    VmresumeAfterVmxoff = 6,
    /// VM entry: a control field is not valid.
    EntryInvalidControls = 7,
    /// VM entry: a host-state field is not valid.
    EntryInvalidHostState = 8,
    VmptrldInvalidAddress = 9,
    VmptrldVmxonPointer = 10,
    VmptrldWrongRevision = 11,
    /// VMREAD or VMWRITE of a field the VMCS does not have.
    UnsupportedField = 12,
    /// VMWRITE of a read-only field.
    ReadOnlyField = 13,
    VmxonInRoot = 15,
    EntryInvalidExecutivePointer = 16,
    EntryNonLaunchedExecutive = 17,
    EntryExecutiveNotVmxon = 18,
    VmcallNonClear = 19,
    VmcallInvalidExitControls = 20,
    VmcallWrongMsegRevision = 22,
    VmxoffDualMonitor = 23,
    VmcallInvalidSmmFeatures = 24,
    EntryInvalidExecutiveControls = 25,
    /// VM entry with events blocked by MOV SS.
    EntryBlockedByMovSs = 26,
    /// INVEPT or INVVPID with an operand that is not valid.
    InvalidInvOperand = 28,
}

impl InstructionError {
    /// Err with the number for one the SDM does not define (0, 14, 21, 27,
    /// or past 28).
    pub const fn from_raw(raw: u32) -> Result<Self, u32> {
        use InstructionError::*;
        Ok(match raw {
            1 => VmcallInRoot,
            2 => VmclearInvalidAddress,
            3 => VmclearVmxonPointer,
            4 => VmlaunchNonClear,
            5 => VmresumeNonLaunched,
            6 => VmresumeAfterVmxoff,
            7 => EntryInvalidControls,
            8 => EntryInvalidHostState,
            9 => VmptrldInvalidAddress,
            10 => VmptrldVmxonPointer,
            11 => VmptrldWrongRevision,
            12 => UnsupportedField,
            13 => ReadOnlyField,
            15 => VmxonInRoot,
            16 => EntryInvalidExecutivePointer,
            17 => EntryNonLaunchedExecutive,
            18 => EntryExecutiveNotVmxon,
            19 => VmcallNonClear,
            20 => VmcallInvalidExitControls,
            22 => VmcallWrongMsegRevision,
            23 => VmxoffDualMonitor,
            24 => VmcallInvalidSmmFeatures,
            25 => EntryInvalidExecutiveControls,
            26 => EntryBlockedByMovSs,
            28 => InvalidInvOperand,
            _ => return Err(raw),
        })
    }
}
