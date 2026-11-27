//! The whole volume at once: every chain a directory names, checked in every
//! FAT copy, and the clusters no file names but a FAT marks used (leaked).

use alloc::vec;
use alloc::vec::Vec;

use crate::bpb::PER_SECTOR;
use crate::dir::ATTR_DIR;
use crate::fat::BAD;
use crate::volume::{UNKNOWN, Volume};
use crate::{Disk, Error, SECTOR, u32_at};

/// What [`Volume::check`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    /// Clusters that a file's or directory's chain names, the root's included.
    pub named: u32,
    /// Clusters that some FAT copy in use marks used and no chain names, and
    /// no copy marks bad.
    pub leaked: u32,
    /// Clusters that some FAT copy in use marks bad. A reclaim keeps them.
    pub bad: u32,
}

/// One bit for each cluster number on the volume.
pub(crate) struct Named {
    bits: Vec<u64>,
    count: u32,
}

impl Named {
    fn new(clusters: u32) -> Self {
        Self {
            bits: vec![0; (clusters as usize + 2).div_ceil(64)],
            count: 0,
        }
    }

    fn mark(&mut self, cluster: u32) -> Result<(), Error> {
        let (word, bit) = (cluster as usize / 64, cluster % 64);
        if self.bits[word] & (1 << bit) != 0 {
            return Err(Error::Corrupt("two chains share a cluster"));
        }
        self.bits[word] |= 1 << bit;
        self.count += 1;
        Ok(())
    }

    fn has(&self, cluster: u32) -> bool {
        self.bits[cluster as usize / 64] & (1 << (cluster % 64)) != 0
    }
}

impl<D: Disk> Volume<D> {
    /// The chain `walk` finds through the first FAT copy in use, refused unless
    /// every other copy in use gives the same one.
    fn agreed(
        &mut self,
        walk: impl Fn(&mut Self, u32) -> Result<Vec<u32>, Error>,
    ) -> Result<Vec<u32>, Error> {
        let mut copies = self.geometry.copies();
        let first = copies.next().expect("at least one FAT");
        let chain = walk(self, first)?;
        for copy in copies {
            if walk(self, copy)? != chain {
                return Err(Error::Corrupt("the FAT copies disagree on a chain"));
            }
        }
        Ok(chain)
    }

    /// Every cluster the root and everything below it names. Each chain must be
    /// sound and the same in every FAT copy in use, and no cluster may be named
    /// twice. A directory's first cluster is marked when its entry is found,
    /// so a directory that names one of its parents, or one named twice, is
    /// refused there, and the directories waiting to be read never outnumber
    /// the volume's clusters.
    pub(crate) fn named(&mut self) -> Result<Named, Error> {
        let mut named = Named::new(self.geometry.clusters);
        let root = self.geometry.root_cluster;
        self.first_of_dir(&mut named, root)?;
        let mut dirs = vec![root];
        while let Some(dir) = dirs.pop() {
            for &cluster in &self.agreed(|v, copy| v.dir_chain(copy, dir))?[1..] {
                named.mark(cluster)?;
            }
            let mut entries = Vec::new();
            let first = self.geometry.first_fat;
            self.scan(first, dir, |_, _, found| {
                entries.push(found);
                None::<()>
            })?;
            for found in entries {
                if found.attr & ATTR_DIR != 0 {
                    self.first_of_dir(&mut named, found.first)?;
                    dirs.push(found.first);
                    continue;
                }
                let (start, size) = (found.first, found.size);
                for cluster in self.agreed(|v, copy| v.file_chain(copy, start, size))? {
                    named.mark(cluster)?;
                }
            }
        }
        Ok(named)
    }

    /// Marks a directory's first cluster, refused unless it is on the volume.
    fn first_of_dir(&mut self, named: &mut Named, first: u32) -> Result<(), Error> {
        if !(2..=self.geometry.last_cluster()).contains(&first) {
            return Err(Error::Corrupt("a first cluster is not on the volume"));
        }
        named.mark(first)
    }

    /// Clusters that some FAT copy in use marks used but `named` does not hold
    /// and no copy marks bad, and how many clusters some copy marks bad. A bad
    /// mark is never freed: it is the only record that the cluster is bad.
    fn leaked(&mut self, named: &Named) -> Result<(Vec<u32>, u32), Error> {
        let g = self.geometry;
        let entries = g.last_cluster() + 1;
        let mut out = Vec::new();
        let mut bad_count = 0;
        let mut sector = [0u8; SECTOR];
        for index in 0..entries.div_ceil(PER_SECTOR) {
            let mut used = [false; PER_SECTOR as usize];
            let mut bad = [false; PER_SECTOR as usize];
            for copy in g.copies() {
                let (lba, _) = g.fat_lba(copy, index * PER_SECTOR);
                self.disk.read(lba, &mut sector)?;
                for slot in 0..PER_SECTOR as usize {
                    let link = u32_at(&sector, slot * 4) & 0x0FFF_FFFF;
                    used[slot] |= link != 0;
                    bad[slot] |= link == BAD;
                }
            }
            for slot in 0..PER_SECTOR as usize {
                let cluster = index * PER_SECTOR + slot as u32;
                if !(2..entries).contains(&cluster) {
                    continue;
                }
                if bad[slot] {
                    bad_count += 1;
                } else if used[slot] && !named.has(cluster) {
                    out.push(cluster);
                }
            }
        }
        Ok((out, bad_count))
    }

    /// Checks the whole volume as [`Volume::replace`] does before it writes,
    /// and counts the clusters a cut left leaked. It writes nothing.
    pub fn check(&mut self) -> Result<Usage, Error> {
        let named = self.named()?;
        let (leaked, bad) = self.leaked(&named)?;
        Ok(Usage {
            named: named.count,
            leaked: leaked.len() as u32,
            bad,
        })
    }

    /// Frees every leaked cluster in every FAT copy in use, after the same
    /// check, and returns how many it freed. No chain names them, so a cut
    /// anywhere loses nothing; FSInfo's count is "unknown" until the frees
    /// are durable, and right again after. Clusters marked bad stay marked.
    pub fn reclaim(&mut self) -> Result<u32, Error> {
        let named = self.named()?;
        let (leaked, bad) = self.leaked(&named)?;
        if !leaked.is_empty() {
            self.fs_info(UNKNOWN)?;
            self.disk.flush()?;
            let frees: Vec<(u32, u32)> = leaked.iter().map(|&c| (c, 0)).collect();
            self.set_links(&frees)?;
            self.disk.flush()?;
        }
        // Set even when nothing leaked, so a cut that left it unknown is undone.
        self.fs_info(self.geometry.clusters - named.count - bad)?;
        self.disk.flush()?;
        Ok(leaked.len() as u32)
    }
}
