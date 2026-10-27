//! The data disk (P2 plan decision 1; ADR 0004 §5): a GPT with a `kai-data`
//! partition at 2 MiB, written and read with `diskmap` (task 2), so the kernel
//! and the case disks find it by the same type GUID; kaifs in it; and a
//! [`BlockDev`] over a file so kaifs reads and writes the image directly.
//!
//! An image without a GPT is taken as kaifs over the whole file.

use crate::blklog::DATA_FIRST;
use crate::image::gpt::guid;
use diskmap::{Disk, Guid, KAI_DATA_TYPE, Spec};
use kaifs::{BLOCK, Block, BlockDev, DevInfo, IoError};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const SECTOR: u64 = diskmap::SECTOR as u64;
const SECTORS_PER_BLOCK: u64 = BLOCK as u64 / SECTOR;
/// The smallest disk: 2 MiB before the partition, 16 MiB of kaifs, the backup
/// array and header.
pub const MIN_BYTES: u64 = 20 << 20;

/// Where kaifs lies in an image: a byte offset and a length in blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub offset: u64,
    pub blocks: u64,
}

/// An image file as `diskmap` reads it.
struct FileDisk<'a> {
    file: &'a mut File,
    sectors: u64,
}

impl Disk for FileDisk<'_> {
    fn sectors(&self) -> u64 {
        self.sectors
    }

    fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), diskmap::Error> {
        self.file
            .seek(SeekFrom::Start(lba * SECTOR))
            .and_then(|_| self.file.read_exact(out))
            .map_err(|_| diskmap::Error::Read)
    }
}

/// An image in memory as `diskmap` reads it.
pub struct Bytes<'a>(pub &'a [u8]);

impl Disk for Bytes<'_> {
    fn sectors(&self) -> u64 {
        self.0.len() as u64 / SECTOR
    }

    fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), diskmap::Error> {
        let at = usize::try_from(lba * SECTOR).map_err(|_| diskmap::Error::Read)?;
        let end = at.checked_add(out.len()).ok_or(diskmap::Error::Read)?;
        let src = self.0.get(at..end).ok_or(diskmap::Error::Read)?;
        out.copy_from_slice(src);
        Ok(())
    }
}

/// Makes `path` a disk of `bytes` (whole MiB, at least [`MIN_BYTES`]) whose GPT
/// has one `kai-data` partition from 2 MiB to the last whole block before the
/// backup array; `name` seeds its GUIDs. Refuses a file that exists unless
/// `force`. Nothing past the GPT is written, so the file stays sparse where the
/// system allows.
pub fn create(path: &Path, bytes: u64, name: &str, force: bool) -> Result<Region, String> {
    if bytes < MIN_BYTES || !bytes.is_multiple_of(1 << 20) {
        return Err(format!(
            "a data disk is whole MiB, at least {} MiB",
            MIN_BYTES >> 20
        ));
    }
    let shown = path.display();
    let mut file = match force {
        true => File::create(path),
        false => OpenOptions::new().write(true).create_new(true).open(path),
    }
    .map_err(|e| format!("{shown}: {e} (an image that exists needs --force)"))?;
    file.set_len(bytes).map_err(|e| format!("{shown}: {e}"))?;
    let sectors = bytes / SECTOR;
    let usable = diskmap::last_usable(sectors) + 1 - DATA_FIRST;
    let last = DATA_FIRST + usable / SECTORS_PER_BLOCK * SECTORS_PER_BLOCK - 1;
    let part = Spec {
        kind: KAI_DATA_TYPE,
        guid: Guid(guid(&format!("{name} kai-data partition"))),
        first: DATA_FIRST,
        last,
        name: "kai-data",
    };
    let disk = Guid(guid(&format!("{name} kai-data disk")));
    let layout = diskmap::layout(sectors, disk, &[part]).map_err(|e| format!("{shown}: {e:?}"))?;
    for (lba, sector) in layout {
        file.seek(SeekFrom::Start(lba * SECTOR))
            .and_then(|_| file.write_all(&sector))
            .map_err(|e| format!("{shown}: {e}"))?;
    }
    file.sync_all().map_err(|e| format!("{shown}: {e}"))?;
    Ok(Region {
        offset: DATA_FIRST * SECTOR,
        blocks: (last + 1 - DATA_FIRST) / SECTORS_PER_BLOCK,
    })
}

/// Where kaifs lies on `disk`, an image of `len` bytes: its `kai-data`
/// partition, as `diskmap` finds it (the backup table if the primary is bad), or
/// the whole image if it has no GPT.
pub fn locate(disk: &mut dyn Disk, len: u64) -> Result<Region, String> {
    let table = match diskmap::read(disk) {
        Ok(table) => table,
        Err(diskmap::Error::NoTable | diskmap::Error::Small) => {
            return Ok(Region {
                offset: 0,
                blocks: len / BLOCK as u64,
            });
        }
        Err(e) => return Err(format!("the GPT does not check: {e:?}")),
    };
    let part = table
        .find(KAI_DATA_TYPE)
        .ok_or("the GPT has no kai-data partition")?;
    if !part.first.is_multiple_of(SECTORS_PER_BLOCK) {
        return Err("the kai-data partition starts off a 4 KiB boundary".into());
    }
    Ok(Region {
        offset: part.first * SECTOR,
        blocks: part.sectors() / SECTORS_PER_BLOCK,
    })
}

/// kaifs's blocks in a region of an image file.
pub struct FileDev {
    file: File,
    region: Region,
}

impl FileDev {
    /// Opens `path` and finds kaifs in it.
    pub fn open(path: &Path, write: bool) -> Result<FileDev, String> {
        let shown = path.display();
        let mut file = OpenOptions::new()
            .read(true)
            .write(write)
            .open(path)
            .map_err(|e| format!("{shown}: {e}"))?;
        let len = file.metadata().map_err(|e| format!("{shown}: {e}"))?.len();
        let mut disk = FileDisk {
            file: &mut file,
            sectors: len / SECTOR,
        };
        let region = locate(&mut disk, len).map_err(|e| format!("{shown}: {e}"))?;
        Ok(FileDev { file, region })
    }

    /// kaifs over `region` of an open file.
    pub fn new(file: File, region: Region) -> FileDev {
        FileDev { file, region }
    }

    fn at(&self, addr: u64) -> Result<u64, IoError> {
        match addr < self.region.blocks {
            true => Ok(self.region.offset + addr * BLOCK as u64),
            false => Err(IoError),
        }
    }
}

impl BlockDev for FileDev {
    fn info(&self) -> DevInfo {
        // The host's page cache holds writes until `sync_data`, which is the
        // flush: a write-back cache that flushes.
        DevInfo {
            blocks: self.region.blocks,
            volatile_cache: true,
            flush: true,
        }
    }

    fn read(&mut self, addr: u64, buf: &mut Block) -> Result<(), IoError> {
        let at = self.at(addr)?;
        self.file.seek(SeekFrom::Start(at)).map_err(|_| IoError)?;
        self.file.read_exact(buf).map_err(|_| IoError)
    }

    fn write(&mut self, addr: u64, data: &Block) -> Result<(), IoError> {
        let at = self.at(addr)?;
        self.file.seek(SeekFrom::Start(at)).map_err(|_| IoError)?;
        self.file.write_all(data).map_err(|_| IoError)
    }

    fn flush(&mut self) -> Result<(), IoError> {
        self.file.sync_data().map_err(|_| IoError)
    }
}
