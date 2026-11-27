//! What the svm host tests share: a table pool and `Shm` frames in host
//! memory, a guest's RAM layout, and a small random generator.

#![allow(dead_code)]

use svm::npt::{Access, Frames, Layout, Region, Table, Tables};

/// Where the fake pool's tables "are" in host physical memory.
pub const POOL_BASE: u64 = 0x1_0000_0000;

/// A pool of up to `limit` tables, at made-up host addresses.
pub struct Pool {
    pub tables: Vec<Box<Table>>,
    pub limit: usize,
}

impl Pool {
    pub fn new(limit: usize) -> Self {
        Self {
            tables: Vec::new(),
            limit,
        }
    }

    fn index(phys: u64) -> Option<usize> {
        let at = phys.checked_sub(POOL_BASE)?;
        (at % 4096 == 0).then_some((at / 4096) as usize)
    }
}

impl Tables for Pool {
    fn alloc(&mut self) -> Option<u64> {
        if self.tables.len() == self.limit {
            return None;
        }
        self.tables.push(Box::new(Table::empty()));
        Some(POOL_BASE + (self.tables.len() as u64 - 1) * 4096)
    }

    fn get(&self, phys: u64) -> Option<&Table> {
        self.tables.get(Self::index(phys)?).map(|t| &**t)
    }

    fn get_mut(&mut self, phys: u64) -> Option<&mut Table> {
        self.tables.get_mut(Self::index(phys)?).map(|t| &mut **t)
    }
}

/// A `Shm` of `bytes`, host contiguous from `base`.
pub struct Contiguous {
    pub base: u64,
    pub bytes: u64,
}

impl Frames for Contiguous {
    fn run(&self, offset: u64, bytes: u64) -> Option<u64> {
        (offset.checked_add(bytes)? <= self.bytes).then_some(self.base + offset)
    }
}

/// A `Shm` whose 4 KiB pages are scattered: page `n` is at
/// `base + n * stride`, so no run longer than a page is contiguous.
pub struct Scattered {
    pub base: u64,
    pub stride: u64,
    pub bytes: u64,
}

impl Frames for Scattered {
    fn run(&self, offset: u64, bytes: u64) -> Option<u64> {
        let fits = offset.checked_add(bytes)? <= self.bytes;
        (fits && bytes == 4096).then_some(self.base + offset / 4096 * self.stride)
    }
}

pub const MIB: u64 = 1 << 20;
pub const GIB: u64 = 1 << 30;

/// A guest of ADR 0006's size: 1,536 MiB of RAM, all below the 3 GiB PCI
/// hole, in a 4 GiB physical space.
pub fn guest_ram() -> Layout {
    let mut layout = Layout::new(4 * GIB, 1536 * MIB).unwrap();
    layout
        .add(Region {
            gpa: 0,
            bytes: 1536 * MIB,
            offset: 0,
            access: Access::RAM,
        })
        .unwrap();
    layout
}

/// SplitMix64: enough randomness for hostile inputs, with no dependency.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}
