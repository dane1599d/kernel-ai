//! Test disks: a sparse disk in memory, a disk that logs its writes so a cut can
//! be replayed at any point, and a small FAT32 image maker (8.3 names, one level
//! of directories, files in contiguous runs).

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::rc::Rc;

use espfat::{Disk, IoError, SECTOR};

pub type Sector = [u8; SECTOR];

/// A sparse disk: sectors never written read as zeros.
#[derive(Clone, Default)]
pub struct Mem {
    pub sectors: BTreeMap<u64, Rc<Sector>>,
    pub size: u64,
}

impl Mem {
    pub fn get(&self, lba: u64) -> Sector {
        self.sectors.get(&lba).map(|s| **s).unwrap_or([0; SECTOR])
    }

    pub fn set(&mut self, lba: u64, data: Sector) {
        self.sectors.insert(lba, Rc::new(data));
    }

    /// Overwrites bytes at byte offset `at` of the disk.
    pub fn patch(&mut self, at: u64, bytes: &[u8]) {
        for (i, &b) in bytes.iter().enumerate() {
            let at = at + i as u64;
            let mut s = self.get(at / SECTOR as u64);
            s[(at % SECTOR as u64) as usize] = b;
            self.set(at / SECTOR as u64, s);
        }
    }

    pub fn u32_at(&self, at: u64) -> u32 {
        let s = self.get(at / SECTOR as u64);
        let o = (at % SECTOR as u64) as usize;
        u32::from_le_bytes(s[o..o + 4].try_into().unwrap())
    }
}

impl Disk for Mem {
    fn read(&mut self, lba: u64, out: &mut Sector) -> Result<(), IoError> {
        if lba >= self.size {
            return Err(IoError);
        }
        *out = self.get(lba);
        Ok(())
    }

    fn write(&mut self, lba: u64, data: &Sector) -> Result<(), IoError> {
        if lba >= self.size {
            return Err(IoError);
        }
        self.set(lba, *data);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        Ok(())
    }
}

/// One write or flush, in the order a disk saw it.
#[derive(Clone)]
pub enum Op {
    Write(u64, Rc<Sector>),
    Flush,
}

/// A disk that applies every write and logs it.
pub struct Logged {
    pub disk: Mem,
    pub log: Vec<Op>,
}

impl Disk for Logged {
    fn read(&mut self, lba: u64, out: &mut Sector) -> Result<(), IoError> {
        self.disk.read(lba, out)
    }

    fn write(&mut self, lba: u64, data: &Sector) -> Result<(), IoError> {
        self.log.push(Op::Write(lba, Rc::new(*data)));
        self.disk.write(lba, data)
    }

    fn flush(&mut self) -> Result<(), IoError> {
        self.log.push(Op::Flush);
        Ok(())
    }
}

impl Logged {
    pub fn writes(&self) -> usize {
        self.log
            .iter()
            .filter(|op| matches!(op, Op::Write(..)))
            .count()
    }

    /// The writes, grouped by the flushes between them.
    pub fn epochs(&self) -> Vec<Vec<(u64, Rc<Sector>)>> {
        let mut out = vec![Vec::new()];
        for op in &self.log {
            match op {
                Op::Write(lba, s) => out.last_mut().unwrap().push((*lba, s.clone())),
                Op::Flush => out.push(Vec::new()),
            }
        }
        out.retain(|e| !e.is_empty());
        out
    }
}

/// `base` with the first `k` logged writes applied, in order.
pub fn cut_at(base: &Mem, log: &[Op], k: usize) -> Mem {
    let mut disk = base.clone();
    let writes = log.iter().filter_map(|op| match op {
        Op::Write(lba, s) => Some((*lba, s.clone())),
        Op::Flush => None,
    });
    for (lba, s) in writes.take(k) {
        disk.sectors.insert(lba, s);
    }
    disk
}

pub const RESERVED: u32 = 32;
pub const SECTORS: u32 = 70_000;
/// The image maker's FAT size for [`SECTORS`] sectors and one-sector clusters.
pub const FAT_SECTORS: u32 = (SECTORS - RESERVED).div_ceil(129);
pub const DATA: u32 = RESERVED + 2 * FAT_SECTORS;
pub const CLUSTERS: u32 = SECTORS - DATA;

/// The byte offset of `cluster`'s entry in FAT copy `copy`.
pub fn fat_at(copy: u32, cluster: u32) -> u64 {
    u64::from(RESERVED + copy * FAT_SECTORS) * SECTOR as u64 + u64::from(cluster) * 4
}

/// The byte offset of `cluster`'s data.
pub fn cluster_at(cluster: u32) -> u64 {
    u64::from(DATA + cluster - 2) * SECTOR as u64
}

/// A file or directory for [`image`]. Names are 8.3, upper case.
pub enum Node {
    File(&'static str, Vec<u8>),
    Dir(&'static str, Vec<Node>),
}

fn name11(name: &str) -> [u8; 11] {
    let mut out = [b' '; 11];
    let (base, ext) = name.split_once('.').unwrap_or((name, ""));
    out[..base.len()].copy_from_slice(base.as_bytes());
    out[8..8 + ext.len()].copy_from_slice(ext.as_bytes());
    out
}

fn entry(name: [u8; 11], attr: u8, first: u32, size: u32) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(&name);
    e[11] = attr;
    e[20..22].copy_from_slice(&((first >> 16) as u16).to_le_bytes());
    e[26..28].copy_from_slice(&(first as u16).to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    e
}

/// A volume's layout: [`SECTORS`] sectors of one-sector clusters by default.
#[derive(Clone, Copy)]
pub struct Layout {
    pub spc: u32,
    pub sectors: u32,
    pub fat_sectors: u32,
}

impl Layout {
    pub const DEFAULT: Self = Self {
        spc: 1,
        sectors: SECTORS,
        fat_sectors: FAT_SECTORS,
    };

    /// `sectors` sectors of `spc`-sector clusters, the FAT sized as the
    /// specification sizes it.
    pub fn new(spc: u32, sectors: u32) -> Self {
        let fat_sectors = (sectors - RESERVED).div_ceil((256 * spc + 2) / 2);
        Self {
            spc,
            sectors,
            fat_sectors,
        }
    }

    fn fat_at(&self, copy: u32, cluster: u32) -> u64 {
        u64::from(RESERVED + copy * self.fat_sectors) * SECTOR as u64 + u64::from(cluster) * 4
    }

    fn cluster_at(&self, cluster: u32) -> u64 {
        let data = RESERVED + 2 * self.fat_sectors;
        u64::from(data + (cluster - 2) * self.spc) * SECTOR as u64
    }

    fn cluster_bytes(&self) -> usize {
        self.spc as usize * SECTOR
    }

    pub fn clusters(&self) -> u32 {
        (self.sectors - RESERVED - 2 * self.fat_sectors) / self.spc
    }
}

struct Maker {
    disk: Mem,
    next: u32,
    l: Layout,
}

impl Maker {
    /// A contiguous run for `bytes`, chained in both FATs.
    fn run(&mut self, bytes: &[u8]) -> u32 {
        let count = bytes.len().div_ceil(self.l.cluster_bytes()).max(1) as u32;
        let first = self.next;
        for c in first..first + count {
            let link = if c + 1 == first + count {
                0x0FFF_FFFF
            } else {
                c + 1
            };
            for copy in 0..2 {
                self.disk.patch(self.l.fat_at(copy, c), &link.to_le_bytes());
            }
        }
        self.disk.patch(self.l.cluster_at(first), bytes);
        self.next += count;
        first
    }

    fn dir(&mut self, at: u32, parent: Option<u32>, nodes: &[Node]) {
        let mut entries = Vec::new();
        if let Some(parent) = parent {
            entries.extend(entry(name11("."), 0x10, at, 0));
            entries.extend(entry(name11(".."), 0x10, parent, 0));
        }
        let mut subdirs = Vec::new();
        for node in nodes {
            match node {
                Node::File(name, bytes) => {
                    let first = if bytes.is_empty() { 0 } else { self.run(bytes) };
                    entries.extend(entry(name11(name), 0x20, first, bytes.len() as u32));
                }
                Node::Dir(name, children) => {
                    let first = self.run(&[0; SECTOR]);
                    entries.extend(entry(name11(name), 0x10, first, 0));
                    subdirs.push((first, children));
                }
            }
        }
        assert!(
            entries.len() <= SECTOR,
            "one sector of entries per directory"
        );
        self.disk.patch(self.l.cluster_at(at), &entries);
        for (first, children) in subdirs {
            self.dir(first, Some(if at == 2 { 0 } else { at }), children);
        }
    }
}

/// A FAT32 volume of [`SECTORS`] one-sector clusters at the start of a disk of
/// the same size, holding `root`.
pub fn image(root: &[Node]) -> Mem {
    image_with(Layout::DEFAULT, root)
}

/// A FAT32 volume laid out as `l` at the start of a disk of its size.
pub fn image_with(l: Layout, root: &[Node]) -> Mem {
    let mut m = Maker {
        disk: Mem {
            sectors: BTreeMap::new(),
            size: u64::from(l.sectors),
        },
        next: 2,
        l,
    };
    let mut boot = [0u8; SECTOR];
    boot[..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    boot[11..13].copy_from_slice(&512u16.to_le_bytes());
    boot[13] = l.spc as u8;
    boot[14..16].copy_from_slice(&(RESERVED as u16).to_le_bytes());
    boot[16] = 2;
    boot[21] = 0xF8;
    boot[32..36].copy_from_slice(&l.sectors.to_le_bytes());
    boot[36..40].copy_from_slice(&l.fat_sectors.to_le_bytes());
    boot[44..48].copy_from_slice(&2u32.to_le_bytes());
    boot[48..50].copy_from_slice(&1u16.to_le_bytes());
    boot[510] = 0x55;
    boot[511] = 0xAA;
    m.disk.set(0, boot);
    for copy in 0..2 {
        m.disk
            .patch(l.fat_at(copy, 0), &0x0FFF_FFF8u32.to_le_bytes());
        m.disk
            .patch(l.fat_at(copy, 1), &0x0FFF_FFFFu32.to_le_bytes());
    }
    let root_cluster = m.run(&[0; SECTOR]);
    m.dir(root_cluster, None, root);
    let free = l.clusters() + 2 - m.next;
    let mut info = [0u8; SECTOR];
    info[..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    info[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    info[488..492].copy_from_slice(&free.to_le_bytes());
    info[492..496].copy_from_slice(&m.next.to_le_bytes());
    info[508..512].copy_from_slice(&0xAA55_0000u32.to_le_bytes());
    m.disk.set(1, info);
    m.disk
}

/// `len` bytes that differ by `seed`.
pub fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed) ^ (i >> 9) as u8)
        .collect()
}

/// A small deterministic generator for the tests' random choices.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}
