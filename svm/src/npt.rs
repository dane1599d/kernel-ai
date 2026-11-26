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
/// The guest physical addresses a 4-level walk reaches: 2^48 (256 TiB).
pub const GUEST_LIMIT: u64 = 1 << 48;

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

// Guest RAM's layout: the guest physical ranges one `Shm` backs, and the
// nested table that maps them (ADR 0006 item 3). The kernel builds it once at
// create; nothing here allocates, so the tables come from a pool sized then.

/// How many ranges one guest's RAM may be split into: RAM below the PCI
/// hole, RAM above 4 GiB, a ROM, and room to spare.
pub const REGIONS: usize = 8;

/// One guest physical range, backed by the `Shm` from `offset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub gpa: u64,
    pub bytes: u64,
    /// Where in the `Shm` the range starts.
    pub offset: u64,
    pub access: Access,
}

impl Region {
    /// Whether `gpa` is in the range.
    pub const fn contains(&self, gpa: u64) -> bool {
        gpa >= self.gpa && gpa - self.gpa < self.bytes
    }
}

/// Why a region, or its mapping, is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The guest's size is not 4 KiB aligned, or past [`GUEST_LIMIT`].
    GuestSize(u64),
    /// The `Shm`'s size is not 4 KiB aligned.
    ShmSize(u64),
    /// The range's address, size or `Shm` offset is not 4 KiB aligned.
    Misaligned(Region),
    /// The range is empty.
    Empty(Region),
    /// The range ends past the guest's physical size.
    PastGuest(Region),
    /// The range ends past the `Shm`.
    PastShm(Region),
    /// The range overlaps region `index`'s guest addresses, or its part of
    /// the `Shm` (two guest addresses for one page).
    Overlap { region: Region, index: usize },
    /// [`REGIONS`] are already in use.
    Full,
    /// The range covers the IOAPIC's or local APIC's registers
    /// ([`crate::exit::IOAPIC`], [`crate::exit::LAPIC`]), which the kernel
    /// emulates: RAM there would hide them.
    Apic(Region),
    /// The `Shm` reports no host page at this offset ([`Frames::run`]).
    Backing(u64),
    /// The table pool is spent ([`Tables::alloc`]).
    NoTables,
    /// The table already maps something at this guest address.
    Mapped(u64),
}

/// The `Shm`'s host frames, as the kernel holds them.
pub trait Frames {
    /// The host physical address of the `bytes` from `offset` (both 4 KiB
    /// multiples) when they are contiguous in host memory, else None. A
    /// single 4 KiB page is always contiguous; None for one is refused.
    fn run(&self, offset: u64, bytes: u64) -> Option<u64>;
}

/// The VM's pool of nested table pages.
pub trait Tables {
    /// A new, empty table's host physical address; None when spent.
    fn alloc(&mut self) -> Option<u64>;
    /// The table at host physical `phys`, one `alloc` gave.
    fn get(&self, phys: u64) -> Option<&Table>;
    fn get_mut(&mut self, phys: u64) -> Option<&mut Table>;
}

/// Guest RAM: up to [`REGIONS`] ranges of one `Shm`, each checked as added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    guest: u64,
    shm: u64,
    regions: [Option<Region>; REGIONS],
}

const PAGE: u64 = Size::Page4K.bytes();

impl Layout {
    /// An empty layout for a guest whose physical addresses end at `guest`
    /// and a `Shm` of `shm` bytes.
    pub const fn new(guest: u64, shm: u64) -> Result<Self, Refusal> {
        if !guest.is_multiple_of(PAGE) || guest > GUEST_LIMIT {
            return Err(Refusal::GuestSize(guest));
        }
        if !shm.is_multiple_of(PAGE) {
            return Err(Refusal::ShmSize(shm));
        }
        Ok(Self {
            guest,
            shm,
            regions: [None; REGIONS],
        })
    }

    /// Adds `region`: refused, with nothing changed, when it is misaligned
    /// or empty, ends past the guest or the `Shm`, covers an APIC's
    /// registers, or overlaps a region already added in guest addresses or
    /// in the `Shm`.
    pub fn add(&mut self, region: Region) -> Result<(), Refusal> {
        let r = region;
        if !(r.gpa | r.bytes | r.offset).is_multiple_of(PAGE) {
            return Err(Refusal::Misaligned(r));
        }
        if r.bytes == 0 {
            return Err(Refusal::Empty(r));
        }
        if r.gpa
            .checked_add(r.bytes)
            .is_none_or(|end| end > self.guest)
        {
            return Err(Refusal::PastGuest(r));
        }
        if r.offset
            .checked_add(r.bytes)
            .is_none_or(|end| end > self.shm)
        {
            return Err(Refusal::PastShm(r));
        }
        let overlaps_apic = [crate::exit::IOAPIC, crate::exit::LAPIC]
            .iter()
            .any(|&base| r.gpa < base + crate::exit::APIC_WINDOW && base < r.gpa + r.bytes);
        if overlaps_apic {
            return Err(Refusal::Apic(r));
        }
        let overlaps = |a: u64, b: u64, len_a: u64, len_b: u64| a < b + len_b && b < a + len_a;
        for (index, old) in self.regions() {
            if overlaps(r.gpa, old.gpa, r.bytes, old.bytes)
                || overlaps(r.offset, old.offset, r.bytes, old.bytes)
            {
                return Err(Refusal::Overlap { region: r, index });
            }
        }
        let slot = self.regions.iter_mut().find(|s| s.is_none());
        *slot.ok_or(Refusal::Full)? = Some(r);
        Ok(())
    }

    /// The regions, in the order added, with their indices.
    pub fn regions(&self) -> impl Iterator<Item = (usize, Region)> + '_ {
        self.regions
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.map(|r| (i, r)))
    }

    /// The region holding `gpa`, if any.
    pub fn find(&self, gpa: u64) -> Option<Region> {
        self.regions().map(|(_, r)| r).find(|r| r.contains(gpa))
    }

    /// The guest's physical size.
    pub const fn guest(&self) -> u64 {
        self.guest
    }

    /// The most tables [`Layout::map`] can take, the root included: every
    /// region in 4 KiB leaves, as scattered frames force, with each region's
    /// PTs, PDs and PDPTs counted as if no other region shared them. A pool
    /// of this many never runs out in `map`; [`tables_for`] is the count
    /// when every leaf is 2 MiB.
    pub fn tables(&self) -> u64 {
        let spans = |r: &Region, size: u64| {
            let first = r.gpa / size;
            let last = (r.gpa + r.bytes - 1) / size;
            last - first + 1
        };
        1 + self
            .regions()
            .map(|(_, r)| spans(&r, 1 << 21) + spans(&r, 1 << 30) + spans(&r, 1 << 39))
            .sum::<u64>()
    }

    /// Maps every region into the nested table at `root` (from
    /// `tables.alloc()`): a 2 MiB leaf where the guest address, the `Shm`
    /// offset and the host run are all 2 MiB aligned, else 4 KiB leaves.
    /// On a refusal the table is part built and the VM is not started.
    pub fn map(
        &self,
        frames: &impl Frames,
        tables: &mut impl Tables,
        root: u64,
    ) -> Result<(), Refusal> {
        let huge = Size::Page2M.bytes();
        for (_, r) in self.regions() {
            let mut done = 0;
            while done < r.bytes {
                let (gpa, offset) = (r.gpa + done, r.offset + done);
                let big = (gpa | offset).is_multiple_of(huge) && r.bytes - done >= huge;
                let run = match big {
                    true => frames.run(offset, huge).filter(|h| h.is_multiple_of(huge)),
                    false => None,
                };
                let (host, size) = match run {
                    Some(host) => (host, Size::Page2M),
                    None => (
                        frames.run(offset, PAGE).ok_or(Refusal::Backing(offset))?,
                        Size::Page4K,
                    ),
                };
                let leaf = NptEntry::leaf(host, size, r.access).ok_or(Refusal::Backing(offset))?;
                install(tables, root, gpa, leaf, size)?;
                done += size.bytes();
            }
        }
        Ok(())
    }
}

/// The table level a leaf of `size` sits at: 1 the PDPT, 2 the PD, 3 the PT.
const fn level(size: Size) -> usize {
    match size {
        Size::Page1G => 1,
        Size::Page2M => 2,
        Size::Page4K => 3,
    }
}

/// Puts `leaf` for `gpa` into the table at `root`, adding the tables on the
/// way from the pool. Refused when something is already mapped there.
pub fn install(
    tables: &mut impl Tables,
    root: u64,
    gpa: u64,
    leaf: NptEntry,
    size: Size,
) -> Result<(), Refusal> {
    let index = indices(gpa);
    let mut table = root;
    for &i in &index[..level(size)] {
        let entry = tables.get(table).ok_or(Refusal::NoTables)?.0[i];
        table = match entry.present() {
            true if entry.huge() => return Err(Refusal::Mapped(gpa)),
            true => entry.address(),
            false => {
                let next = tables.alloc().ok_or(Refusal::NoTables)?;
                let link = NptEntry::table(next).ok_or(Refusal::NoTables)?;
                tables.get_mut(table).ok_or(Refusal::NoTables)?.0[i] = link;
                next
            }
        };
    }
    let slot = &mut tables.get_mut(table).ok_or(Refusal::NoTables)?.0[index[level(size)]];
    if slot.present() {
        return Err(Refusal::Mapped(gpa));
    }
    *slot = leaf;
    Ok(())
}

/// Walks the table at `root` for `gpa`, as the CPU would: the host physical
/// address and the leaf's rights, or None where the guest would #NPF.
pub fn translate(tables: &impl Tables, root: u64, gpa: u64) -> Option<(u64, Access)> {
    if gpa >= GUEST_LIMIT {
        return None;
    }
    let index = indices(gpa);
    let mut table = root;
    for (level, &i) in index.iter().enumerate() {
        let entry = tables.get(table)?.0[i];
        if !entry.present() {
            return None;
        }
        let size = match level {
            1 if entry.huge() => Some(Size::Page1G),
            2 if entry.huge() => Some(Size::Page2M),
            3 => Some(Size::Page4K),
            _ => None,
        };
        if let Some(size) = size {
            let access = Access {
                write: entry.writable(),
                execute: entry.executable(),
            };
            return Some((entry.address() + (gpa & (size.bytes() - 1)), access));
        }
        table = entry.address();
    }
    None
}
