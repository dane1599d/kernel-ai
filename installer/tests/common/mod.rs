//! Disks for the installer's tests: a sparse disk whose writes stay pending
//! until a flush, small FAT32 volumes, and a running disk built from them.

#![allow(dead_code)]

use std::collections::BTreeMap;

use diskmap::{Guid, Spec, Table};
use installer::{Committed, Disk, IoError, KINDS, NAMES, PARTITION_MIB, Source};
use sha2::{Digest, Sha256};

pub const S: usize = 512;
pub type Sector = [u8; S];

/// A sparse disk. A write stays pending, readable but not durable, until a
/// flush; every write and flush is logged, for the cut tests to replay.
#[derive(Clone, Default)]
pub struct Mem {
    pub sectors: u64,
    pub durable: BTreeMap<u64, Sector>,
    pub pending: BTreeMap<u64, Sector>,
    /// Every write as (first LBA, sectors), and `None` for a flush.
    pub log: Vec<Option<(u64, Vec<Sector>)>>,
}

impl Mem {
    pub fn new(sectors: u64) -> Self {
        Self {
            sectors,
            ..Self::default()
        }
    }

    pub fn get(&self, lba: u64) -> Sector {
        self.pending
            .get(&lba)
            .or_else(|| self.durable.get(&lba))
            .copied()
            .unwrap_or([0; S])
    }

    /// Sets a sector durably, unlogged: for building disks.
    pub fn put(&mut self, lba: u64, s: Sector) {
        self.pending.remove(&lba);
        set(&mut self.durable, lba, s);
    }

    /// What a power cut leaves now: the durable sectors only.
    pub fn cut(&self) -> Mem {
        Mem {
            sectors: self.sectors,
            durable: self.durable.clone(),
            ..Mem::default()
        }
    }

    pub fn writes(&self) -> impl Iterator<Item = &(u64, Vec<Sector>)> {
        self.log.iter().flatten()
    }

    /// The SHA-256 of `sectors` from `lba`.
    pub fn hash(&self, lba: u64, sectors: u64) -> [u8; 32] {
        let mut h = Sha256::new();
        for at in lba..lba + sectors {
            h.update(self.get(at));
        }
        h.finalize().into()
    }
}

/// Sets `lba` in `map`, keeping zeros absent.
pub fn set(map: &mut BTreeMap<u64, Sector>, lba: u64, s: Sector) {
    match s == [0; S] {
        true => map.remove(&lba),
        false => map.insert(lba, s),
    };
}

impl Disk for Mem {
    fn sectors(&self) -> u64 {
        self.sectors
    }

    fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), IoError> {
        let n = (out.len() / S) as u64;
        if lba + n > self.sectors {
            return Err(IoError);
        }
        for (i, chunk) in out.as_chunks_mut::<S>().0.iter_mut().enumerate() {
            *chunk = self.get(lba + i as u64);
        }
        Ok(())
    }

    fn write(&mut self, lba: u64, data: &[u8]) -> Result<(), IoError> {
        let n = (data.len() / S) as u64;
        if lba + n > self.sectors {
            return Err(IoError);
        }
        let sectors: Vec<Sector> = data.as_chunks::<S>().0.to_vec();
        for (i, s) in sectors.iter().enumerate() {
            self.pending.insert(lba + i as u64, *s);
        }
        self.log.push(Some((lba, sectors)));
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        for (lba, s) in std::mem::take(&mut self.pending) {
            set(&mut self.durable, lba, s);
        }
        self.log.push(None);
        Ok(())
    }
}

impl diskmap::Disk for Mem {
    fn sectors(&self) -> u64 {
        self.sectors
    }

    fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), diskmap::Error> {
        Disk::read(self, lba, out).map_err(|_| diskmap::Error::Read)
    }
}

/// The FAT32 volumes' size: just above FAT32's floor of 65,525 clusters.
pub const VOLUME: u64 = 70_000;
const RESERVED: u64 = 32;
const FAT: u64 = 543;
const DATA_CLUSTERS: u32 = 40;

fn cluster_lba(cluster: u32) -> u64 {
    RESERVED + 2 * FAT + u64::from(cluster - 2)
}

fn entry(name: &[u8; 11], attr: u8, first: u32, size: u32) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(name);
    e[11] = attr;
    e[20..22].copy_from_slice(&((first >> 16) as u16).to_le_bytes());
    e[26..28].copy_from_slice(&(first as u16).to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    e
}

pub fn pattern(lba: u64, seed: u8) -> Sector {
    let mut s = [0u8; S];
    for (i, b) in s.iter_mut().enumerate() {
        *b = ((lba as u8).wrapping_mul(31) ^ (i as u8) ^ seed) | 1;
    }
    s
}

/// A FAT32 volume of [`VOLUME`] one-sector clusters at `at` on `disk`: a
/// `DATA.BIN` of `seed`'s bytes, and `boot/slot` naming `release_seq` when
/// given.
pub fn fat32(disk: &mut Mem, at: u64, seed: u8, release_seq: Option<u32>) {
    let mut boot = [0u8; S];
    boot[..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    boot[11..13].copy_from_slice(&512u16.to_le_bytes());
    boot[13] = 1;
    boot[14..16].copy_from_slice(&(RESERVED as u16).to_le_bytes());
    boot[16] = 2;
    boot[32..36].copy_from_slice(&(VOLUME as u32).to_le_bytes());
    boot[36..40].copy_from_slice(&(FAT as u32).to_le_bytes());
    boot[44..48].copy_from_slice(&2u32.to_le_bytes());
    boot[510] = 0x55;
    boot[511] = 0xAA;
    disk.put(at, boot);

    const END: u32 = 0x0FFF_FFFF;
    let record = release_seq.map(|n| format!("kai-slot 1\nrelease_seq {n}\nepoch 1\n"));
    // Clusters: 2 the root, 3 `boot`, 4 `boot/slot`, then DATA.BIN.
    let data_first = 5;
    let mut links = vec![(0, 0x0FFF_FFF8), (1, END), (2, END), (3, END), (4, END)];
    for c in data_first..data_first + DATA_CLUSTERS {
        let next = if c + 1 == data_first + DATA_CLUSTERS {
            END
        } else {
            c + 1
        };
        links.push((c, next));
    }
    let mut fat = [[0u8; S]; 1];
    for (c, v) in links {
        let off = c as usize * 4;
        fat[0][off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    for copy in 0..2 {
        disk.put(at + RESERVED + copy * FAT, fat[0]);
    }
    let mut root = [0u8; S];
    root[..32].copy_from_slice(&entry(b"BOOT       ", 0x10, 3, 0));
    root[32..64].copy_from_slice(&entry(
        b"DATA    BIN",
        0x20,
        data_first,
        DATA_CLUSTERS * S as u32,
    ));
    disk.put(at + cluster_lba(2), root);
    let mut dir = [0u8; S];
    dir[..32].copy_from_slice(&entry(b".          ", 0x10, 3, 0));
    dir[32..64].copy_from_slice(&entry(b"..         ", 0x10, 0, 0));
    if let Some(r) = &record {
        dir[64..96].copy_from_slice(&entry(b"SLOT       ", 0x20, 4, r.len() as u32));
        let mut s = [0u8; S];
        s[..r.len()].copy_from_slice(r.as_bytes());
        disk.put(at + cluster_lba(4), s);
    }
    disk.put(at + cluster_lba(3), dir);
    for c in data_first..data_first + DATA_CLUSTERS {
        let lba = at + cluster_lba(c);
        disk.put(lba, pattern(lba, seed));
    }
    // Past the volume: bytes no install may copy.
    disk.put(at + VOLUME, [0xEE; S]);
}

/// A target with 64 MiB for `kai-data`.
pub fn disk_sectors() -> u64 {
    installer::data_first() + 64 * 2048 + 34
}

/// A running disk and its source: the ADR's layout, a FAT32 volume in the
/// ESP and `kai-r`, slot volumes in `kai-a` (release `seed`) and `kai-b`
/// (release `seed + 1`), and bytes in `kai-data`. The committed slot is
/// `committed`, `KAI_A_TYPE` or `KAI_B_TYPE`.
pub struct Running {
    pub disk: Mem,
    pub table: Table,
    pub committed: Committed,
}

impl Running {
    pub fn new(seed: u8, committed: Guid) -> Self {
        let mut disk = Mem::new(disk_sectors());
        let mut first = installer::FIRST_LBA;
        let mut specs = Vec::new();
        let id = format!("running-{seed}");
        for (n, name) in NAMES.iter().enumerate() {
            let count = PARTITION_MIB.get(n).map_or(64 * 2048, |mib| mib * 2048);
            specs.push(Spec {
                kind: KINDS[n],
                guid: Guid(installer::unique(&id, name)),
                first,
                last: first + count - 1,
                name,
            });
            first += count;
        }
        let disk_guid = Guid(installer::unique(&id, "disk"));
        for (lba, bytes) in diskmap::layout(disk.sectors, disk_guid, &specs).unwrap() {
            for (i, s) in bytes.as_chunks::<S>().0.iter().enumerate() {
                disk.put(lba + i as u64, *s);
            }
        }
        let seq = |slot: u8| u32::from(seed) + u32::from(slot);
        fat32(&mut disk, specs[0].first, seed, None);
        fat32(&mut disk, specs[1].first, seed + 1, None);
        fat32(&mut disk, specs[2].first, seed + 2, Some(seq(0)));
        fat32(&mut disk, specs[3].first, seed + 3, Some(seq(1)));
        disk.put(specs[4].first, [0xDD; S]);
        let table = diskmap::read(&mut disk).unwrap();
        let slot = table.find(committed).unwrap().first;
        let committed = Committed {
            kind: committed,
            sha256: disk.hash(slot, VOLUME),
            release_seq: seq(u8::from(committed == diskmap::KAI_B_TYPE)),
        };
        Self {
            disk,
            table,
            committed,
        }
    }

    pub fn source(&self) -> Source<'_> {
        Source {
            table: &self.table,
            committed: self.committed,
        }
    }
}
