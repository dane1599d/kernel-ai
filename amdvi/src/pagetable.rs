//! The host I/O page table (doc 48882 rev 3.10, section 2.2.3): a tree of
//! 4 KiB tables of 512 little-endian `u64`s, `levels` deep, that the device
//! table entry's root points at. Level `n` is indexed by IOVA bits
//! `12 + 9n - 1 : 12 + 9(n - 1)`, so a 4-level table covers 48 bits.
//!
//! An entry (PDE or PTE):
//!
//! | Bits  | Field                                                  |
//! |-------|--------------------------------------------------------|
//! | 0     | PR: present                                            |
//! | 11:9  | Next Level: the level of the table it points at; 0 in a PTE for a 4 KiB page, 7 for a larger page |
//! | 51:12 | The next table's address, or the page's                |
//! | 61    | IR: reads allowed (every level's must allow it)        |
//! | 62    | IW: writes allowed (likewise)                          |
//!
//! Only 4 KiB pages are written. A PDE allows both reads and writes, so a
//! page's own PTE decides.
//!
//! The IOMMU caches PDEs, so a table taken out of the tree may still be
//! walked through an old IOVA. Such a table is never freed here: it goes to
//! [`DmaAlloc::retire`], and the caller frees it only after an
//! INVALIDATE_IOMMU_PAGES with PDE set for the domain and a COMPLETION_WAIT
//! after it have completed ([`crate::command`]). The unmapped pages need the
//! same invalidation before their frames are reused.

use crate::{ADDRESS_MASK, bit, field, page_address};

/// Entries in one table.
pub const ENTRIES: usize = 512;
/// Bytes in a page and in a table.
pub const PAGE: u64 = 4096;

const PR: u64 = 1;
const NEXT_LEVEL: u32 = 9;
const IR: u32 = 61;
const IW: u32 = 62;

/// The kernel's DMA memory for page tables: frames that are zeroed when
/// given, 4 KiB aligned and below bit 52, read and written by entry.
pub trait DmaAlloc {
    /// A zeroed frame's physical address, or `None` when memory is short.
    fn alloc(&mut self) -> Option<u64>;
    /// Takes back a table [`DmaAlloc::alloc`] gave, now out of the tree.
    /// It must not be reused until the domain's PDE cache is invalidated
    /// and a COMPLETION_WAIT after that has completed: the IOMMU may still
    /// walk it through an old IOVA until then.
    fn retire(&mut self, phys: u64);
    /// Entry `index` of the table at `phys`.
    fn read(&self, phys: u64, index: usize) -> u64;
    fn write(&mut self, phys: u64, index: usize, value: u64);
}

/// What a page allows a device to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Access {
    pub read: bool,
    pub write: bool,
}

impl Access {
    pub const READ: Self = Self {
        read: true,
        write: false,
    };
    pub const READ_WRITE: Self = Self {
        read: true,
        write: true,
    };
}

/// Why a map or an unmap changed nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// Levels outside 1-6.
    Levels(u8),
    /// An address not 4 KiB aligned, or no pages.
    Unaligned,
    /// Past what the levels cover, or a physical address above bit 51.
    OutOfRange,
    /// A page that allows neither reads nor writes.
    NoAccess,
    /// This IOVA is mapped already.
    Mapped(u64),
    /// This IOVA is not mapped.
    NotMapped(u64),
    /// A table could not be allocated.
    NoMemory,
    /// An entry on this IOVA's path is not one this crate writes.
    Corrupt(u64),
}

/// One domain's page table: its root and depth. The tables live in the
/// [`DmaAlloc`]'s memory, which every call is given.
#[derive(Debug, PartialEq, Eq)]
pub struct IoPageTable {
    root: u64,
    levels: u8,
}

/// Where a walk ended.
enum Walk {
    /// The page's PTE: its table and index.
    Page(u64, usize),
    /// Not mapped: the deepest table reached, its level, and the index there.
    Missing(u64, u8, usize),
}

fn index(iova: u64, level: u8) -> usize {
    field(
        iova,
        12 + 9 * (u32::from(level) - 1),
        12 + 9 * u32::from(level) - 1,
    ) as usize
        & (ENTRIES - 1)
}

impl IoPageTable {
    /// An empty table of `levels` levels: one zeroed root.
    pub fn new(a: &mut impl DmaAlloc, levels: u8) -> Result<Self, MapError> {
        if !(1..=6).contains(&levels) {
            return Err(MapError::Levels(levels));
        }
        let root = a.alloc().ok_or(MapError::NoMemory)?;
        Ok(Self { root, levels })
    }

    /// The root, for the device table entry.
    pub fn root(&self) -> u64 {
        self.root
    }

    pub fn levels(&self) -> u8 {
        self.levels
    }

    /// One past the highest IOVA the levels cover.
    pub fn limit(&self) -> Option<u64> {
        1u64.checked_shl(12 + 9 * u32::from(self.levels))
    }

    fn walk(&self, a: &impl DmaAlloc, iova: u64) -> Result<Walk, MapError> {
        let mut table = self.root;
        for level in (1..=self.levels).rev() {
            let at = index(iova, level);
            let entry = a.read(table, at);
            if entry & PR == 0 {
                return Ok(Walk::Missing(table, level, at));
            }
            if level == 1 {
                return match field(entry, NEXT_LEVEL, NEXT_LEVEL + 2) {
                    0 => Ok(Walk::Page(table, at)),
                    _ => Err(MapError::Corrupt(iova)),
                };
            }
            if field(entry, NEXT_LEVEL, NEXT_LEVEL + 2) != u64::from(level - 1) {
                return Err(MapError::Corrupt(iova));
            }
            table = entry & ADDRESS_MASK;
        }
        unreachable!("level 1 ends the walk")
    }

    /// Where `iova` goes, and what it allows; `None` when it is not mapped
    /// or its path is not one this crate wrote.
    pub fn translate(&self, a: &impl DmaAlloc, iova: u64) -> Option<(u64, Access)> {
        if self.limit().is_some_and(|limit| iova >= limit) {
            return None;
        }
        match self.walk(a, iova & !(PAGE - 1)) {
            Ok(Walk::Page(table, at)) => {
                let entry = a.read(table, at);
                let access = Access {
                    read: bit(entry, IR),
                    write: bit(entry, IW),
                };
                Some(((entry & ADDRESS_MASK) | (iova & (PAGE - 1)), access))
            }
            _ => None,
        }
    }

    /// The IOVAs of `pages` pages from `iova`, checked against the levels.
    fn span(&self, iova: u64, pages: usize) -> Result<u64, MapError> {
        if !iova.is_multiple_of(PAGE) || pages == 0 {
            return Err(MapError::Unaligned);
        }
        let end = (pages as u64)
            .checked_mul(PAGE)
            .and_then(|len| iova.checked_add(len))
            .ok_or(MapError::OutOfRange)?;
        match self.limit() {
            Some(limit) if end > limit => Err(MapError::OutOfRange),
            _ => Ok(end),
        }
    }

    /// Maps `pages` pages from `iova` to the frames from `phys`, all or
    /// none: nothing in the range may be mapped already, and a shortage of
    /// tables part way undoes the pages it mapped. The tables that undoing
    /// empties are retired, so an `Err(NoMemory)` needs the same
    /// invalidation as an unmap before they are freed.
    pub fn map(
        &mut self,
        a: &mut impl DmaAlloc,
        iova: u64,
        phys: u64,
        pages: usize,
        access: Access,
    ) -> Result<(), MapError> {
        let end = self.span(iova, pages)?;
        if !phys.is_multiple_of(PAGE) {
            return Err(MapError::Unaligned);
        }
        let last = (pages as u64 - 1)
            .checked_mul(PAGE)
            .and_then(|len| phys.checked_add(len));
        if !last.is_some_and(page_address) {
            return Err(MapError::OutOfRange);
        }
        if !access.read && !access.write {
            return Err(MapError::NoAccess);
        }
        for page in (iova..end).step_by(PAGE as usize) {
            if let Walk::Page(..) = self.walk(a, page)? {
                return Err(MapError::Mapped(page));
            }
        }
        for (n, page) in (iova..end).step_by(PAGE as usize).enumerate() {
            if let Err(e) = self.map_one(a, page, phys + n as u64 * PAGE, access) {
                // Tables made for this page before the shortage are empty.
                self.prune(a, page);
                for done in (iova..page).step_by(PAGE as usize) {
                    let _ = self.unmap_one(a, done);
                }
                return Err(e);
            }
        }
        Ok(())
    }

    fn map_one(
        &mut self,
        a: &mut impl DmaAlloc,
        iova: u64,
        phys: u64,
        access: Access,
    ) -> Result<(), MapError> {
        loop {
            match self.walk(a, iova)? {
                Walk::Page(..) => return Err(MapError::Mapped(iova)),
                Walk::Missing(table, 1, at) => {
                    let pte =
                        PR | phys | u64::from(access.read) << IR | u64::from(access.write) << IW;
                    a.write(table, at, pte);
                    return Ok(());
                }
                Walk::Missing(table, level, at) => {
                    let next = a.alloc().ok_or(MapError::NoMemory)?;
                    let pde = PR | u64::from(level - 1) << NEXT_LEVEL | next | 1 << IR | 1 << IW;
                    a.write(table, at, pde);
                }
            }
        }
    }

    /// Unmaps `pages` pages from `iova`, all or none: each must be mapped.
    /// A table left empty is retired, except the root. `Ok(true)` when any
    /// was: the invalidation must then have PDE set.
    pub fn unmap(
        &mut self,
        a: &mut impl DmaAlloc,
        iova: u64,
        pages: usize,
    ) -> Result<bool, MapError> {
        let end = self.span(iova, pages)?;
        for page in (iova..end).step_by(PAGE as usize) {
            if let Walk::Missing(..) = self.walk(a, page)? {
                return Err(MapError::NotMapped(page));
            }
        }
        let mut retired = false;
        for page in (iova..end).step_by(PAGE as usize) {
            retired |= self.unmap_one(a, page);
        }
        Ok(retired)
    }

    /// Clears `iova`'s PTE and prunes its path: whether a table was retired.
    fn unmap_one(&mut self, a: &mut impl DmaAlloc, iova: u64) -> bool {
        if let Ok(Walk::Page(table, at)) = self.walk(a, iova) {
            a.write(table, at, 0);
        }
        self.prune(a, iova)
    }

    /// Retires the empty tables on `iova`'s path, deepest first, and clears
    /// the entries that pointed at them. The root stays. Whether any was.
    fn prune(&mut self, a: &mut impl DmaAlloc, iova: u64) -> bool {
        // (parent table, index there, the table it points at), from the root.
        let mut path = [(0u64, 0usize, 0u64); 6];
        let mut depth = 0;
        let mut table = self.root;
        for level in (2..=self.levels).rev() {
            let at = index(iova, level);
            let entry = a.read(table, at);
            if entry & PR == 0 || field(entry, NEXT_LEVEL, NEXT_LEVEL + 2) != u64::from(level - 1) {
                break;
            }
            let next = entry & ADDRESS_MASK;
            path[depth] = (table, at, next);
            depth += 1;
            table = next;
        }
        let mut retired = false;
        for &(parent, at, table) in path[..depth].iter().rev() {
            if (0..ENTRIES).any(|i| a.read(table, i) != 0) {
                break;
            }
            a.write(parent, at, 0);
            a.retire(table);
            retired = true;
        }
        retired
    }

    /// Retires every table, the root included, once no device table entry
    /// points at the root any more. Only entries this crate writes are
    /// followed: a PDE whose Next Level is not one below its own is not.
    pub fn destroy(self, a: &mut impl DmaAlloc) {
        fn retire(a: &mut impl DmaAlloc, table: u64, level: u8) {
            if level > 1 {
                for at in 0..ENTRIES {
                    let entry = a.read(table, at);
                    let next = field(entry, NEXT_LEVEL, NEXT_LEVEL + 2);
                    if entry & PR != 0 && next == u64::from(level - 1) {
                        retire(a, entry & ADDRESS_MASK, level - 1);
                    }
                }
            }
            a.retire(table);
        }
        retire(a, self.root, self.levels);
    }
}
