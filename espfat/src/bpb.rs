//! The boot sector's BIOS parameter block, checked before any of it is used.

use crate::{Error, SECTOR, u16_at, u32_at};

/// FAT32's lower bound on clusters. A volume with fewer is FAT12 or FAT16.
const MIN_CLUSTERS: u32 = 65_525;
/// The highest cluster number FAT32 allows. 0x0FFF_FFF7 marks a bad cluster.
const MAX_CLUSTER: u32 = 0x0FFF_FFF6;
/// FAT entries in one sector.
pub(crate) const PER_SECTOR: u32 = (SECTOR / 4) as u32;

/// A FAT32 volume's layout, read from its boot sector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    /// The volume's first sector on the disk.
    pub start: u64,
    /// The volume's size in sectors.
    pub sectors: u32,
    pub sectors_per_cluster: u32,
    pub reserved_sectors: u32,
    /// FAT copies on the volume.
    pub fats: u32,
    pub fat_sectors: u32,
    pub root_cluster: u32,
    /// Data clusters: they are numbered 2 to `clusters + 1`.
    pub clusters: u32,
    /// The FSInfo sector, if the volume names a usable one.
    pub fs_info: Option<u32>,
    /// The FAT copies in use: all of them, or only the active one when the
    /// volume turns mirroring off (BPB_ExtFlags bit 7).
    pub first_fat: u32,
    pub used_fats: u32,
}

impl Geometry {
    /// Reads the boot sector of a volume that starts at `start` and may hold at
    /// most `room` sectors.
    pub fn parse(s: &[u8; SECTOR], start: u64, room: u64) -> Result<Self, Error> {
        let bad = Error::NotFat32;
        if s[510] != 0x55 || s[511] != 0xAA {
            return Err(bad("no boot sector signature"));
        }
        if usize::from(u16_at(s, 11)) != SECTOR {
            return Err(bad("sectors are not 512 bytes"));
        }
        let sectors_per_cluster = u32::from(s[13]);
        if !sectors_per_cluster.is_power_of_two() {
            return Err(bad("sectors per cluster is not a power of two"));
        }
        let reserved_sectors = u32::from(u16_at(s, 14));
        if reserved_sectors == 0 {
            return Err(bad("no reserved sectors"));
        }
        let fats = u32::from(s[16]);
        if !(1..=2).contains(&fats) {
            return Err(bad("not one or two FATs"));
        }
        if u16_at(s, 17) != 0 || u16_at(s, 19) != 0 || u16_at(s, 22) != 0 {
            return Err(bad("FAT12 or FAT16 fields are set"));
        }
        let sectors = u32_at(s, 32);
        if u64::from(sectors) > room {
            return Err(bad("the volume is larger than its partition"));
        }
        let fat_sectors = u32_at(s, 36);
        if fat_sectors == 0 {
            return Err(bad("the FAT has no sectors"));
        }
        let data_start = u64::from(reserved_sectors) + u64::from(fats) * u64::from(fat_sectors);
        if data_start >= u64::from(sectors) {
            return Err(bad("the FATs fill the volume"));
        }
        let clusters = ((u64::from(sectors) - data_start) / u64::from(sectors_per_cluster)) as u32;
        if clusters < MIN_CLUSTERS {
            return Err(bad("too few clusters for FAT32"));
        }
        if clusters > MAX_CLUSTER - 1 {
            return Err(bad("too many clusters for FAT32"));
        }
        if u64::from(fat_sectors) * u64::from(PER_SECTOR) < u64::from(clusters) + 2 {
            return Err(bad("the FAT is too small for the clusters"));
        }
        let flags = u16_at(s, 40);
        let (first_fat, used_fats) = match flags & 0x80 {
            0 => (0, fats),
            _ => (u32::from(flags & 0x0F), 1),
        };
        if first_fat >= fats {
            return Err(bad("the active FAT is not on the volume"));
        }
        if u16_at(s, 42) != 0 {
            return Err(bad("not FAT32 version 0.0"));
        }
        let root_cluster = u32_at(s, 44);
        if !(2..=clusters + 1).contains(&root_cluster) {
            return Err(bad("the root directory's cluster is not on the volume"));
        }
        let info = u32::from(u16_at(s, 48));
        let fs_info = (info != 0 && info != 0xFFFF && info < reserved_sectors).then_some(info);
        Ok(Self {
            start,
            sectors,
            sectors_per_cluster,
            reserved_sectors,
            fats,
            fat_sectors,
            root_cluster,
            clusters,
            fs_info,
            first_fat,
            used_fats,
        })
    }

    pub fn cluster_bytes(&self) -> u32 {
        self.sectors_per_cluster * SECTOR as u32
    }

    /// The highest cluster number on the volume.
    pub fn last_cluster(&self) -> u32 {
        self.clusters + 1
    }

    /// The disk sector of sector `n` of `cluster`.
    pub(crate) fn cluster_lba(&self, cluster: u32, n: u32) -> u64 {
        let data =
            u64::from(self.reserved_sectors) + u64::from(self.fats) * u64::from(self.fat_sectors);
        self.start
            + data
            + u64::from(cluster - 2) * u64::from(self.sectors_per_cluster)
            + u64::from(n)
    }

    /// The disk sector of FAT copy `copy` that holds `cluster`'s entry, and the
    /// entry's byte offset in it.
    pub(crate) fn fat_lba(&self, copy: u32, cluster: u32) -> (u64, usize) {
        let sector = u64::from(self.reserved_sectors)
            + u64::from(copy) * u64::from(self.fat_sectors)
            + u64::from(cluster / PER_SECTOR);
        (self.start + sector, (cluster % PER_SECTOR) as usize * 4)
    }

    /// The FAT copies in use, in order.
    pub(crate) fn copies(&self) -> core::ops::Range<u32> {
        self.first_fat..self.first_fat + self.used_fats
    }
}
