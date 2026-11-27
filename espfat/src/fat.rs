//! The file allocation table: links between clusters, the chains they make, and
//! free clusters.

use alloc::vec::Vec;

use crate::bpb::PER_SECTOR;
use crate::volume::Volume;
use crate::{Disk, Error, SECTOR, u32_at};

/// The low 28 bits of an entry. The top 4 are reserved and kept as they are.
const MASK: u32 = 0x0FFF_FFFF;
const BAD: u32 = 0x0FFF_FFF7;
/// The end-of-chain mark espfat writes. Readers take 0x0FFF_FFF8 and above.
pub(crate) const END: u32 = 0x0FFF_FFFF;
/// FAT's limit on a directory: 65,536 entries of 32 bytes.
const MAX_DIR_BYTES: u32 = 65_536 * 32;

/// What a FAT entry says about its cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Link {
    Free,
    Next(u32),
    End,
}

impl<D: Disk> Volume<D> {
    /// The entry for `cluster` in FAT copy `copy`.
    pub(crate) fn link(&mut self, copy: u32, cluster: u32) -> Result<Link, Error> {
        let (lba, at) = self.geometry.fat_lba(copy, cluster);
        let raw = u32_at(self.sector(lba)?, at) & MASK;
        match raw {
            0 => Ok(Link::Free),
            BAD => Err(Error::Corrupt("a chain reaches a bad cluster")),
            n if n >= 0x0FFF_FFF8 => Ok(Link::End),
            n if (2..=self.geometry.last_cluster()).contains(&n) => Ok(Link::Next(n)),
            _ => Err(Error::Corrupt("a chain leaves the volume")),
        }
    }

    fn first(&self, cluster: u32) -> Result<u32, Error> {
        match (2..=self.geometry.last_cluster()).contains(&cluster) {
            true => Ok(cluster),
            false => Err(Error::Corrupt("a first cluster is not on the volume")),
        }
    }

    /// The clusters of a file of `size` bytes that starts at `first`, read
    /// through FAT copy `copy`. The chain must be exactly as long as the size
    /// needs, and an empty file has no chain. A size the volume cannot hold is
    /// refused before any link is read.
    pub(crate) fn file_chain(
        &mut self,
        copy: u32,
        first: u32,
        size: u32,
    ) -> Result<Vec<u32>, Error> {
        let need = size.div_ceil(self.geometry.cluster_bytes());
        if need > self.geometry.clusters {
            return Err(Error::Corrupt("a file is larger than the volume"));
        }
        if need == 0 {
            return match first {
                0 => Ok(Vec::new()),
                _ => Err(Error::Corrupt("an empty file has clusters")),
            };
        }
        let mut chain = Vec::new();
        let mut at = self.first(first)?;
        loop {
            chain.push(at);
            match self.link(copy, at)? {
                Link::Next(next) if chain.len() < need as usize => at = next,
                Link::Next(_) => return Err(Error::Corrupt("a chain is longer than its file")),
                Link::End if chain.len() == need as usize => return Ok(chain),
                Link::End => return Err(Error::Corrupt("a chain is shorter than its file")),
                Link::Free => return Err(Error::Corrupt("a chain reaches a free cluster")),
            }
        }
    }

    /// The clusters of the directory that starts at `first`, through FAT copy
    /// `copy`. Following at most FAT's largest directory also ends a chain that
    /// loops.
    pub(crate) fn dir_chain(&mut self, copy: u32, first: u32) -> Result<Vec<u32>, Error> {
        let most = MAX_DIR_BYTES.div_ceil(self.geometry.cluster_bytes()) as usize;
        let mut chain = Vec::new();
        let mut at = self.first(first)?;
        loop {
            if chain.len() == most {
                return Err(Error::Corrupt("a directory is larger than FAT allows"));
            }
            chain.push(at);
            match self.link(copy, at)? {
                Link::Next(next) => at = next,
                Link::End => return Ok(chain),
                Link::Free => return Err(Error::Corrupt("a chain reaches a free cluster")),
            }
        }
    }

    /// The first `need` clusters that every FAT copy in use marks free, and how
    /// many clusters the first copy marks free in all.
    pub(crate) fn free_clusters(&mut self, need: u32) -> Result<(Vec<u32>, u32), Error> {
        let g = self.geometry;
        let entries = g.last_cluster() + 1;
        let mut found = Vec::new();
        let mut free = 0u32;
        let mut copy = [0u8; SECTOR];
        let mut both = [true; PER_SECTOR as usize];
        for index in 0..entries.div_ceil(PER_SECTOR) {
            both.fill(true);
            for (n, fat) in g.copies().enumerate() {
                let (lba, _) = g.fat_lba(fat, index * PER_SECTOR);
                self.disk.read(lba, &mut copy)?;
                for (slot, is_free) in both.iter_mut().enumerate() {
                    let cluster = index * PER_SECTOR + slot as u32;
                    if !(2..entries).contains(&cluster) {
                        *is_free = false;
                        continue;
                    }
                    let empty = u32_at(&copy, slot * 4) & MASK == 0;
                    if n == 0 && empty {
                        free += 1;
                    }
                    *is_free &= empty;
                }
            }
            for (slot, is_free) in both.iter().enumerate() {
                if *is_free && (found.len() as u32) < need {
                    found.push(index * PER_SECTOR + slot as u32);
                }
            }
        }
        match found.len() as u32 == need {
            true => Ok((found, free)),
            false => Err(Error::NoSpace),
        }
    }

    /// Sets each `(cluster, value)` in every FAT copy in use, keeping each
    /// entry's top 4 bits. Every FAT sector is written once, in cluster order.
    pub(crate) fn set_links(&mut self, changes: &[(u32, u32)]) -> Result<(), Error> {
        let mut changes = changes.to_vec();
        changes.sort_unstable_by_key(|&(cluster, _)| cluster);
        for fat in self.geometry.copies() {
            let mut rest = &changes[..];
            while let Some(&(cluster, _)) = rest.first() {
                let index = cluster / PER_SECTOR;
                let same = rest
                    .iter()
                    .take_while(|(c, _)| c / PER_SECTOR == index)
                    .count();
                let (lba, _) = self.geometry.fat_lba(fat, cluster);
                let mut sector = *self.sector(lba)?;
                for &(cluster, value) in &rest[..same] {
                    let at = (cluster % PER_SECTOR) as usize * 4;
                    let kept = u32_at(&sector, at) & !MASK;
                    sector[at..at + 4].copy_from_slice(&(kept | (value & MASK)).to_le_bytes());
                }
                self.put(lba, &sector)?;
                rest = &rest[same..];
            }
        }
        Ok(())
    }
}
