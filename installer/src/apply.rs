//! Writing a plan: the device step, over any disk.

use alloc::vec;

use crate::{BLOCK_SECTORS, Error, Plan, SECTOR, Step, format_data};

const S: usize = SECTOR as usize;
/// Sectors copied or zeroed in one write: 64 KiB.
const PIECE: u64 = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoError;

/// A disk of 512-byte sectors.
pub trait Disk {
    fn sectors(&self) -> u64;
    /// Fills `out`, whole sectors, from `lba`.
    fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), IoError>;
    /// Writes `data`, whole sectors, at `lba`.
    fn write(&mut self, lba: u64, data: &[u8]) -> Result<(), IoError>;
    /// Makes every write before it durable.
    fn flush(&mut self) -> Result<(), IoError>;
}

impl From<IoError> for Error {
    fn from(_: IoError) -> Self {
        Error::Io
    }
}

/// The sectors of the FAT volume whose boot sector is `boot`, refused unless
/// it is a 512-byte-sector FAT boot sector of at most `most` sectors.
fn volume_sectors(boot: &[u8], most: u64) -> Result<u64, Error> {
    let u16_at = |at: usize| u16::from_le_bytes([boot[at], boot[at + 1]]);
    if boot[510..512] != [0x55, 0xAA] || u16_at(11) != SECTOR as u16 {
        return Err(Error::Source("a running partition holds no FAT volume"));
    }
    let total = match u16_at(19) {
        0 => u64::from(u32::from_le_bytes([boot[32], boot[33], boot[34], boot[35]])),
        n => u64::from(n),
    };
    match (1..=most).contains(&total) {
        true => Ok(total),
        false => Err(Error::Source("a running volume does not fit its partition")),
    }
}

/// Writes `plan` to `target`, copying from `source`, the running disk. Every
/// volume to copy is checked first, so a source that cannot be copied writes
/// nothing.
pub fn apply(plan: &Plan, source: &mut dyn Disk, target: &mut dyn Disk) -> Result<(), Error> {
    if target.sectors() != plan.sectors {
        return Err(Error::Target);
    }
    let mut boot = [0u8; S];
    for step in &plan.steps {
        if let Step::Copy { from, most, .. } = *step {
            source.read(from, &mut boot)?;
            volume_sectors(&boot, most)?;
        }
    }
    let mut piece = vec![0u8; PIECE as usize * S];
    for step in &plan.steps {
        match *step {
            Step::Zero { lba, sectors } => {
                piece.fill(0);
                let mut done = 0;
                while done < sectors {
                    let n = (sectors - done).min(PIECE);
                    target.write(lba + done, &piece[..n as usize * S])?;
                    done += n;
                }
            }
            Step::Copy { from, to, most } => {
                source.read(from, &mut boot)?;
                let sectors = volume_sectors(&boot, most)?;
                let mut done = 0;
                while done < sectors {
                    let n = (sectors - done).min(PIECE);
                    let bytes = &mut piece[..n as usize * S];
                    source.read(from + done, bytes)?;
                    target.write(to + done, bytes)?;
                    done += n;
                }
            }
            Step::Format {
                lba,
                sectors,
                uuid,
                now,
            } => {
                let window = Window {
                    disk: &mut *target,
                    first: lba,
                    blocks: sectors / BLOCK_SECTORS,
                };
                format_data(window, uuid, "kai-data", now).map_err(Error::Data)?;
            }
            Step::Write { lba, ref bytes } => target.write(lba, bytes)?,
            Step::Flush => target.flush()?,
        }
    }
    Ok(())
}

/// The kaifs blocks of a partition: `blocks` of 4 KiB from LBA `first`.
pub struct Window<'a> {
    pub disk: &'a mut dyn Disk,
    pub first: u64,
    pub blocks: u64,
}

impl kaifs::BlockDev for Window<'_> {
    fn info(&self) -> kaifs::DevInfo {
        kaifs::DevInfo {
            blocks: self.blocks,
            volatile_cache: true,
            flush: true,
        }
    }

    fn read(&mut self, addr: u64, buf: &mut kaifs::Block) -> Result<(), kaifs::IoError> {
        if addr >= self.blocks {
            return Err(kaifs::IoError);
        }
        let lba = self.first + addr * BLOCK_SECTORS;
        self.disk.read(lba, buf).map_err(|_| kaifs::IoError)
    }

    fn write(&mut self, addr: u64, data: &kaifs::Block) -> Result<(), kaifs::IoError> {
        if addr >= self.blocks {
            return Err(kaifs::IoError);
        }
        let lba = self.first + addr * BLOCK_SECTORS;
        self.disk.write(lba, data).map_err(|_| kaifs::IoError)
    }

    fn flush(&mut self) -> Result<(), kaifs::IoError> {
        self.disk.flush().map_err(|_| kaifs::IoError)
    }
}
