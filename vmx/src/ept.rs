//! Extended page tables (SDM vol. 3C, 29.3): guest physical addresses mapped
//! to host physical ones by a 4-level table rooted at the EPT pointer (VMCS
//! 0x201A). Unlike a nested page table, an entry's rights are read, write and
//! execute bits of its own, with no present bit: an entry with none of them
//! is not present, and some combinations are not valid at all (an EPT
//! misconfiguration exit, 29.3.3.1). The builders here make only valid
//! entries; [`EptEntry::check`] reads one the kernel did not build.
//!
//! Every entry: bit 0 read, 1 write, 2 execute (supervisor execute with
//! mode-based execute control), 8 accessed, 10 user execute, the address in
//! 51:12. A leaf adds 5:3 the memory type, 6 ignore PAT, 9 dirty and 63
//! suppress #VE; bit 7 makes a PDPTE or PDE a 1 GiB or 2 MiB leaf.

pub const READ: u64 = 1 << 0;
pub const WRITE: u64 = 1 << 1;
pub const EXECUTE: u64 = 1 << 2;
const MEMORY_TYPE_SHIFT: u32 = 3;
const MEMORY_TYPE: u64 = 0b111 << MEMORY_TYPE_SHIFT;
/// Ignore the guest's PAT: the leaf's memory type is the access's.
pub const IGNORE_PAT: u64 = 1 << 6;
/// A 1 GiB (PDPTE) or 2 MiB (PDE) leaf.
pub const LARGE: u64 = 1 << 7;
/// Set by the CPU, with A/D flags enabled in the EPT pointer.
pub const ACCESSED: u64 = 1 << 8;
pub const DIRTY: u64 = 1 << 9;
/// User-mode execute, with mode-based execute control.
pub const USER_EXECUTE: u64 = 1 << 10;
/// Suppress #VE: an EPT violation through this leaf exits rather than
/// raising #VE in the guest.
pub const SUPPRESS_VE: u64 = 1 << 63;
/// A 4 KiB entry's address bits, 51:12.
pub const ADDRESS: u64 = 0x000F_FFFF_FFFF_F000;
/// The highest physical address the format holds, 2^52 - 1. A CPU's own
/// MAXPHYADDR may be lower; [`EptEntry::check`] takes it.
pub const MAX_PHYSICAL: u64 = (1 << 52) - 1;

/// INVEPT's types (30.3): one EPT pointer's mappings, or every one.
pub const INVEPT_SINGLE_CONTEXT: u64 = 1;
pub const INVEPT_GLOBAL: u64 = 2;

/// A leaf's memory type (bits 5:3), and the EPT pointer's (bits 2:0, where
/// only UC and WB are allowed). Types 2, 3 and 7 are reserved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MemoryType {
    Uncacheable = 0,
    WriteCombining = 1,
    WriteThrough = 4,
    WriteProtected = 5,
    WriteBack = 6,
}

impl MemoryType {
    pub const fn from_raw(raw: u8) -> Result<Self, BadEntry> {
        Ok(match raw {
            0 => Self::Uncacheable,
            1 => Self::WriteCombining,
            4 => Self::WriteThrough,
            5 => Self::WriteProtected,
            6 => Self::WriteBack,
            _ => return Err(BadEntry::MemoryType(raw)),
        })
    }
}

/// What a guest may do with a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

impl Access {
    /// RAM: read, write and execute, as a guest's own RAM is.
    pub const RAM: Self = Self {
        read: true,
        write: true,
        execute: true,
    };
    /// Read-only and not executable: a ROM the guest reads as data.
    pub const READ: Self = Self {
        read: true,
        write: false,
        execute: false,
    };

    const fn bits(self) -> u64 {
        ((self.read as u64) * READ)
            | ((self.write as u64) * WRITE)
            | ((self.execute as u64) * EXECUTE)
    }
}

/// The size of the region one entry maps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Page4K,
    Page2M,
    Page1G,
}

impl Size {
    pub const fn bytes(self) -> u64 {
        match self {
            Self::Page4K => 1 << 12,
            Self::Page2M => 1 << 21,
            Self::Page1G => 1 << 30,
        }
    }
}

/// Why an entry, or an EPT pointer, is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadEntry {
    /// An address not aligned to its size, or past the physical limit.
    Address(u64),
    /// Write without read, which is a misconfiguration.
    WriteWithoutRead,
    /// Execute without read, which needs the CPU's execute-only support
    /// (IA32_VMX_EPT_VPID_CAP bit 0); the kernel's guests do not use it.
    ExecuteOnly,
    /// No right at all: that is [`EptEntry::EMPTY`], not a mapping.
    NoAccess,
    /// A reserved memory type (2, 3 or 7), or one an EPT pointer cannot
    /// have (only UC and WB).
    MemoryType(u8),
    /// A reserved bit is set for the entry's level.
    Reserved(u64),
    /// Bit 7 on a PML4E, which has no large form.
    LargeAtTop(u64),
    /// An EPT pointer's walk length is not 4 or 5 levels.
    WalkLength(u64),
}

/// What a valid entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// No right: a guest access through it is an EPT violation.
    NotPresent,
    /// It points at the next level's table.
    Table,
    /// It maps a region of this size.
    Leaf(Size),
}

/// One 64-bit EPT entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct EptEntry(pub u64);

impl EptEntry {
    /// An entry that maps nothing.
    pub const EMPTY: Self = Self(0);

    /// An entry pointing at the next level's table at host physical `next`,
    /// with read, write and execute: the leaf below decides.
    pub const fn table(next: u64) -> Result<Self, BadEntry> {
        match aligned(next, Size::Page4K, MAX_PHYSICAL) {
            true => Ok(Self(next | READ | WRITE | EXECUTE)),
            false => Err(BadEntry::Address(next)),
        }
    }

    /// A leaf mapping a region of `size` at host physical `host`, aligned to
    /// `size`, with `access` and `memory`, the guest's PAT ignored (the
    /// kernel picks each region's type, and a guest's PAT may not make its
    /// RAM uncached behind the host's back). Suppress #VE is set, so every
    /// violation exits.
    pub const fn leaf(
        host: u64,
        size: Size,
        access: Access,
        memory: MemoryType,
    ) -> Result<Self, BadEntry> {
        if !aligned(host, size, MAX_PHYSICAL) {
            return Err(BadEntry::Address(host));
        }
        if let Err(e) = rights(access.bits()) {
            return Err(e);
        }
        let mut entry =
            host | access.bits() | (memory as u64) << MEMORY_TYPE_SHIFT | IGNORE_PAT | SUPPRESS_VE;
        if !matches!(size, Size::Page4K) {
            entry |= LARGE;
        }
        Ok(Self(entry))
    }

    /// Reads an entry at `level` (4 PML4, 3 PDPT, 2 PD, 1 PT) as the CPU
    /// would, for a CPU with `phys_bits` of physical address: what it is, or
    /// why it is a misconfiguration. An entry with no right is not present
    /// whatever its other bits. Only user execute (bit 10) set is refused as
    /// execute-only: with mode-based execute control it is one, and without
    /// it the CPU ignores the bit, so no entry the kernel builds sets it.
    pub const fn check(self, level: u8, phys_bits: u8) -> Result<Kind, BadEntry> {
        let e = self.0;
        let r = e & (READ | WRITE | EXECUTE);
        if r == 0 && e & USER_EXECUTE == 0 {
            return Ok(Kind::NotPresent);
        }
        if let Err(err) = rights(r) {
            // Only user execute: the same as execute-only.
            return Err(match (r, err) {
                (0, _) => BadEntry::ExecuteOnly,
                (_, err) => err,
            });
        }
        let bits = if phys_bits > 52 { 52 } else { phys_bits };
        // Bits 51:MAXPHYADDR are reserved at every level.
        let above = ADDRESS & !((1u64 << bits) - 1);
        if e & above != 0 {
            return Err(BadEntry::Reserved(e));
        }
        let leaf_size = match (level, e & LARGE != 0) {
            (1, _) => Some(Size::Page4K),
            (2, true) => Some(Size::Page2M),
            (3, true) => Some(Size::Page1G),
            (2 | 3, false) => None,
            (4 | 5, true) => return Err(BadEntry::LargeAtTop(e)),
            (4 | 5, false) => None,
            _ => return Err(BadEntry::Reserved(e)),
        };
        match leaf_size {
            None => {
                // A table entry: bits 7:3 are reserved.
                if e & 0xF8 != 0 {
                    return Err(BadEntry::Reserved(e));
                }
                Ok(Kind::Table)
            }
            Some(size) => {
                if let Err(err) = MemoryType::from_raw(((e & MEMORY_TYPE) >> 3) as u8) {
                    return Err(err);
                }
                // A large leaf's address bits below its size are reserved.
                if !matches!(size, Size::Page4K) && e & ADDRESS & (size.bytes() - 1) != 0 {
                    return Err(BadEntry::Reserved(e));
                }
                Ok(Kind::Leaf(size))
            }
        }
    }

    pub const fn readable(self) -> bool {
        self.0 & READ != 0
    }

    pub const fn writable(self) -> bool {
        self.0 & WRITE != 0
    }

    pub const fn executable(self) -> bool {
        self.0 & EXECUTE != 0
    }

    pub const fn large(self) -> bool {
        self.0 & LARGE != 0
    }

    /// The host physical address it holds (bits 51:12; a large leaf's are
    /// its region's base, as [`EptEntry::check`] refuses lower bits).
    pub const fn address(self) -> u64 {
        self.0 & ADDRESS
    }

    /// The leaf's memory type, Err if reserved.
    pub const fn memory_type(self) -> Result<MemoryType, BadEntry> {
        MemoryType::from_raw(((self.0 & MEMORY_TYPE) >> 3) as u8)
    }
}

/// Refuses the read, write and execute combinations that are not a mapping
/// or need a capability the kernel does not use.
const fn rights(r: u64) -> Result<(), BadEntry> {
    if r == 0 {
        Err(BadEntry::NoAccess)
    } else if r & WRITE != 0 && r & READ == 0 {
        Err(BadEntry::WriteWithoutRead)
    } else if r & READ == 0 {
        Err(BadEntry::ExecuteOnly)
    } else {
        Ok(())
    }
}

/// Whether `address` is `size` aligned and no higher than `max`.
const fn aligned(address: u64, size: Size, max: u64) -> bool {
    address & (size.bytes() - 1) == 0 && address <= max
}

/// The EPT pointer (25.6.11): bits 2:0 the walks' memory type (UC or WB),
/// 5:3 the walk length less 1, 6 enables the accessed and dirty flags, 7
/// enforces supervisor shadow-stack rights, 11:8 reserved, the PML4's (or
/// PML5's) address in 51:12.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Eptp {
    pub root: u64,
    /// 4, or 5 with 5-level EPT.
    pub levels: u8,
    pub memory: MemoryType,
    pub accessed_dirty: bool,
}

const EPTP_AD: u64 = 1 << 6;

impl Eptp {
    /// A 4-level EPT pointer at `root`, walked write-back, without A/D flags.
    pub const fn new(root: u64) -> Result<Self, BadEntry> {
        if !aligned(root, Size::Page4K, MAX_PHYSICAL) {
            return Err(BadEntry::Address(root));
        }
        Ok(Self {
            root,
            levels: 4,
            memory: MemoryType::WriteBack,
            accessed_dirty: false,
        })
    }

    pub const fn encode(self) -> u64 {
        self.root
            | ((self.accessed_dirty as u64) * EPTP_AD)
            | ((self.levels as u64 - 1) & 0b111) << 3
            | self.memory as u64
    }

    /// Reads an EPT pointer the kernel did not build (a nested guest's, or
    /// the VMCS's own), with the checks VM entry makes (26.2.1.1). Bit 7 is
    /// accepted but not kept: the kernel does not use it.
    pub const fn decode(raw: u64) -> Result<Self, BadEntry> {
        // Bits 11:8 and 63:52 are reserved.
        if raw & !(ADDRESS | 0xFF) != 0 {
            return Err(BadEntry::Reserved(raw));
        }
        let memory = match raw & 0b111 {
            0 => MemoryType::Uncacheable,
            6 => MemoryType::WriteBack,
            t => return Err(BadEntry::MemoryType(t as u8)),
        };
        let levels = match (raw >> 3) & 0b111 {
            3 => 4,
            4 => 5,
            _ => return Err(BadEntry::WalkLength(raw)),
        };
        Ok(Self {
            root: raw & ADDRESS,
            levels,
            memory,
            accessed_dirty: raw & EPTP_AD != 0,
        })
    }
}

/// The four table indices a 4-level walk of guest physical `gpa` uses: PML4,
/// PDPT, PD, PT, nine bits each from bit 39 down.
pub const fn indices(gpa: u64) -> [usize; 4] {
    [
        ((gpa >> 39) & 0x1FF) as usize,
        ((gpa >> 30) & 0x1FF) as usize,
        ((gpa >> 21) & 0x1FF) as usize,
        ((gpa >> 12) & 0x1FF) as usize,
    ]
}

/// One EPT table: 512 entries, a 4 KiB page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C, align(4096))]
pub struct Table(pub [EptEntry; 512]);

impl Table {
    pub const fn empty() -> Self {
        Self([EptEntry::EMPTY; 512])
    }
}
