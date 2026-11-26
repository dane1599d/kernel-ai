//! Nested page table entries (APM vol. 2, 15.25): the guest's physical
//! addresses mapped to host physical ones by a 4-level table in the long-mode
//! format, rooted at the VMCB's N_CR3. Every nested access counts as a user
//! access (15.25.5), so each entry on a path has U/S set; a leaf's R/W and NX
//! give the guest's rights. Guest RAM is one `Shm` object mapped in full at
//! create (ADR 0006 item 3), so the kernel builds these once.

/// Present.
pub const P: u64 = 1 << 0;
/// Writable.
pub const RW: u64 = 1 << 1;
/// User: required on every nested entry.
pub const US: u64 = 1 << 2;
/// Page write-through (PAT index bit 0).
pub const PWT: u64 = 1 << 3;
/// Page cache disable (PAT index bit 1).
pub const PCD: u64 = 1 << 4;
/// Accessed, set by the CPU.
pub const A: u64 = 1 << 5;
/// Dirty, set by the CPU on a leaf.
pub const D: u64 = 1 << 6;
/// Page size: a 2 MiB (PDE) or 1 GiB (PDPE) leaf.
pub const PS: u64 = 1 << 7;
/// No execute; needs EFER.NXE in the host, as the kernel sets (ADR 0012).
pub const NX: u64 = 1 << 63;
/// A 4 KiB entry's address bits, 51:12.
pub const ADDRESS: u64 = 0x000F_FFFF_FFFF_F000;
/// The highest physical address the format holds, 2^52 - 1.
pub const MAX_PHYSICAL: u64 = (1 << 52) - 1;

/// The power-on PAT, `0x0007_0406_0007_0406`: entry 0 write-back, 1
/// write-through, 2 UC-, 3 UC, and again from 4. A guest's G_PAT starts as
/// this (VMCB save area 0x268), so its own PAT index 0 means write-back.
pub const PAT_POWER_ON: u64 = 0x0007_0406_0007_0406;

/// What a guest may do with a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access {
    pub write: bool,
    pub execute: bool,
}

impl Access {
    /// RAM: read, write and execute, as a guest's own RAM is.
    pub const RAM: Self = Self {
        write: true,
        execute: true,
    };
    /// Read-only and not executable: a ROM the guest reads as data.
    pub const READ: Self = Self {
        write: false,
        execute: false,
    };
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

/// One 64-bit nested page table entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct NptEntry(pub u64);

impl NptEntry {
    /// An entry that maps nothing: a guest access through it is a #NPF.
    pub const EMPTY: Self = Self(0);

    /// An entry pointing at the next level's table at host physical `next`,
    /// with every right: the leaf below decides. None if `next` is not 4 KiB
    /// aligned or past [`MAX_PHYSICAL`].
    pub const fn table(next: u64) -> Option<Self> {
        match aligned(next, Size::Page4K) {
            true => Some(Self(next | P | RW | US)),
            false => None,
        }
    }

    /// A leaf mapping a region of `size` at host physical `host`, aligned
    /// to `size`, with `access` and write-back memory (PAT index 0). None for
    /// an unaligned or too-high address.
    pub const fn leaf(host: u64, size: Size, access: Access) -> Option<Self> {
        if !aligned(host, size) {
            return None;
        }
        let mut entry = host | P | US;
        if access.write {
            entry |= RW;
        }
        if !access.execute {
            entry |= NX;
        }
        if !matches!(size, Size::Page4K) {
            entry |= PS;
        }
        Some(Self(entry))
    }

    pub const fn present(self) -> bool {
        self.0 & P != 0
    }

    pub const fn writable(self) -> bool {
        self.0 & RW != 0
    }

    pub const fn executable(self) -> bool {
        self.0 & NX == 0
    }

    /// A 2 MiB or 1 GiB leaf (meaningful at those levels only).
    pub const fn huge(self) -> bool {
        self.0 & PS != 0
    }

    /// The host physical address it holds: the next table's, or the page's.
    pub const fn address(self) -> u64 {
        // A 2 MiB or 1 GiB leaf's bit 12 is its PAT bit, which `leaf` never
        // sets; it is masked so the address is the region's base either way.
        match self.huge() {
            true => self.0 & ADDRESS & !(1 << 12),
            false => self.0 & ADDRESS,
        }
    }
}

/// Whether `address` is `size` aligned and fits the 52-bit format.
const fn aligned(address: u64, size: Size) -> bool {
    address & (size.bytes() - 1) == 0 && address <= MAX_PHYSICAL
}

/// N_CR3 for a nested table whose top level is at host physical `pml4`:
/// None if it is not 4 KiB aligned. PWT and PCD stay 0 (write-back walks).
pub const fn ncr3(pml4: u64) -> Option<u64> {
    match aligned(pml4, Size::Page4K) {
        true => Some(pml4),
        false => None,
    }
}

/// The four table indices a walk of guest physical `gpa` uses: PML4, PDPT,
/// PD, PT, nine bits each from bit 39 down.
pub const fn indices(gpa: u64) -> [usize; 4] {
    [
        ((gpa >> 39) & 0x1FF) as usize,
        ((gpa >> 30) & 0x1FF) as usize,
        ((gpa >> 21) & 0x1FF) as usize,
        ((gpa >> 12) & 0x1FF) as usize,
    ]
}

/// One nested table: 512 entries, a 4 KiB page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C, align(4096))]
pub struct Table(pub [NptEntry; 512]);

impl Table {
    pub const fn empty() -> Self {
        Self([NptEntry::EMPTY; 512])
    }
}

/// How many 4 KiB tables mapping `bytes` of guest RAM from guest physical 0
/// in 2 MiB leaves takes: one PML4, one PDPT a 512 GiB, one PD a 1 GiB. The
/// kernel sizes the VM's table pages with it at create, so mapping never
/// allocates while the guest runs.
pub const fn tables_for(bytes: u64) -> u64 {
    let gib = bytes.div_ceil(Size::Page1G.bytes());
    let pdpts = gib.div_ceil(512);
    1 + pdpts + gib
}
