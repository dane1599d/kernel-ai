//! A mounted volume: finding, reading and replacing files.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::bpb::Geometry;
use crate::dir::{ATTR_DIR, Found};
use crate::fat::END;
use crate::{Disk, Error, SECTOR, u32_at};

const FS_INFO_LEAD: u32 = 0x4161_5252;
const FS_INFO_STRUCT: u32 = 0x6141_7272;
const FS_INFO_TRAIL: u32 = 0xAA55_0000;
/// FSInfo's "not known" free count and next-free hint.
const UNKNOWN: u32 = 0xFFFF_FFFF;

/// A FAT32 volume on a disk.
pub struct Volume<D> {
    pub(crate) disk: D,
    pub(crate) geometry: Geometry,
    /// The FAT copy that chains are read through.
    pub(crate) read_fat: u32,
    /// The last sector read or written, so a run of entries in one sector costs
    /// one read.
    cache: Option<(u64, Box<[u8; SECTOR]>)>,
}

impl<D: Disk> Volume<D> {
    /// Mounts the volume that starts at sector `start` of `disk` and may take at
    /// most `room` sectors.
    pub fn open(mut disk: D, start: u64, room: u64) -> Result<Self, Error> {
        let mut boot = [0u8; SECTOR];
        disk.read(start, &mut boot)?;
        let geometry = Geometry::parse(&boot, start, room)?;
        Ok(Self {
            disk,
            read_fat: geometry.first_fat,
            geometry,
            cache: None,
        })
    }

    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// Reads chains through FAT copy `copy` instead of the first one in use. A
    /// check after a cut reads every file through each copy.
    pub fn read_through(&mut self, copy: u32) -> Result<(), Error> {
        if !self.geometry.copies().contains(&copy) {
            return Err(Error::NotFound);
        }
        self.read_fat = copy;
        Ok(())
    }

    /// Gives the disk back.
    pub fn into_disk(self) -> D {
        self.disk
    }

    pub(crate) fn sector(&mut self, lba: u64) -> Result<&[u8; SECTOR], Error> {
        if self.cache.as_ref().is_none_or(|(at, _)| *at != lba) {
            let mut buf = Box::new([0u8; SECTOR]);
            self.disk.read(lba, &mut buf)?;
            self.cache = Some((lba, buf));
        }
        Ok(&self.cache.as_ref().expect("just filled").1)
    }

    pub(crate) fn put(&mut self, lba: u64, data: &[u8; SECTOR]) -> Result<(), Error> {
        self.cache = None;
        self.disk.write(lba, data)?;
        Ok(())
    }

    /// The entry `path` names: components split by `/`, from the root.
    fn find(&mut self, path: &str) -> Result<Found, Error> {
        let path = path.strip_prefix('/').unwrap_or(path);
        let mut dir = self.geometry.root_cluster;
        let mut parts = path.split('/').peekable();
        loop {
            let part = parts.next().ok_or(Error::BadPath)?;
            if part.is_empty() || part == "." || part == ".." {
                return Err(Error::BadPath);
            }
            let found = self.lookup(dir, part)?;
            if parts.peek().is_none() {
                return Ok(found);
            }
            if found.attr & ATTR_DIR == 0 {
                return Err(Error::NotDir);
            }
            dir = found.first;
        }
    }

    fn find_file(&mut self, path: &str) -> Result<Found, Error> {
        let found = self.find(path)?;
        match found.attr & ATTR_DIR {
            0 => Ok(found),
            _ => Err(Error::IsDir),
        }
    }

    /// The contents of the file at `path`, refused as [`Error::TooLarge`] when
    /// it is longer than `max` bytes.
    pub fn read_file(&mut self, path: &str, max: usize) -> Result<Vec<u8>, Error> {
        let found = self.find_file(path)?;
        let size = found.size as usize;
        if size > max {
            return Err(Error::TooLarge);
        }
        let chain = self.file_chain(found.first, found.size)?;
        let g = self.geometry;
        let mut out = Vec::with_capacity(size);
        let mut buf = [0u8; SECTOR];
        'read: for cluster in chain {
            for n in 0..g.sectors_per_cluster {
                if out.len() == size {
                    break 'read;
                }
                self.disk.read(g.cluster_lba(cluster, n), &mut buf)?;
                let take = (size - out.len()).min(SECTOR);
                out.extend_from_slice(&buf[..take]);
            }
        }
        Ok(out)
    }

    /// Replaces the contents of the existing file at `path` with `bytes`,
    /// keeping its name, attributes and times. A cut at any point leaves the
    /// old contents or the new ones (see the crate's docs for the order).
    pub fn replace(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        let size = u32::try_from(bytes.len()).map_err(|_| Error::TooLarge)?;
        let found = self.find_file(path)?;
        let old = self.file_chain(found.first, found.size)?;
        let g = self.geometry;
        let need = size.div_ceil(g.cluster_bytes());
        let (new, free) = self.free_clusters(need)?;

        // 1. The new contents, in clusters no FAT copy uses.
        self.fs_info(UNKNOWN)?;
        let mut buf = [0u8; SECTOR];
        let mut sectors = bytes.chunks(SECTOR);
        for &cluster in &new {
            for n in 0..g.sectors_per_cluster {
                let chunk = sectors.next().unwrap_or(&[]);
                buf.fill(0);
                buf[..chunk.len()].copy_from_slice(chunk);
                self.put(g.cluster_lba(cluster, n), &buf)?;
            }
        }
        self.disk.flush()?;

        // 2. Their chain, in every FAT copy.
        let links: Vec<(u32, u32)> = new
            .iter()
            .enumerate()
            .map(|(i, &c)| (c, new.get(i + 1).copied().unwrap_or(END)))
            .collect();
        self.set_links(&links)?;
        self.disk.flush()?;

        // 3. The switch: one sector.
        let first = new.first().copied().unwrap_or(0);
        let mut sector = *self.sector(found.lba)?;
        let e = &mut sector[found.at..found.at + 32];
        e[20..22].copy_from_slice(&((first >> 16) as u16).to_le_bytes());
        e[26..28].copy_from_slice(&(first as u16).to_le_bytes());
        e[28..32].copy_from_slice(&size.to_le_bytes());
        self.put(found.lba, &sector)?;
        self.disk.flush()?;

        // 4. The old chain, freed.
        let freed: Vec<(u32, u32)> = old.iter().map(|&c| (c, 0)).collect();
        self.set_links(&freed)?;
        self.disk.flush()?;

        // 5. FSInfo's count again, once the FATs are durable.
        self.fs_info(free - need + old.len() as u32)?;
        self.disk.flush()?;
        Ok(())
    }

    /// Sets FSInfo's free count to `free` and its next-free hint to unknown,
    /// when the volume has an FSInfo sector whose signatures hold.
    fn fs_info(&mut self, free: u32) -> Result<(), Error> {
        let Some(at) = self.geometry.fs_info else {
            return Ok(());
        };
        let lba = self.geometry.start + u64::from(at);
        let mut s = *self.sector(lba)?;
        let signed = u32_at(&s, 0) == FS_INFO_LEAD
            && u32_at(&s, 484) == FS_INFO_STRUCT
            && u32_at(&s, 508) == FS_INFO_TRAIL;
        if !signed || (u32_at(&s, 488) == free && u32_at(&s, 492) == UNKNOWN) {
            return Ok(());
        }
        s[488..492].copy_from_slice(&free.to_le_bytes());
        s[492..496].copy_from_slice(&UNKNOWN.to_le_bytes());
        self.put(lba, &s)
    }
}
