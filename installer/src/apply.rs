//! Writing a plan: the device step, over any disk.

use alloc::vec;
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

use crate::{BLOCK_SECTORS, Error, Expect, Plan, SECTOR, Step, current, format_data};

const S: usize = SECTOR as usize;
/// Sectors copied, zeroed or read back in one go: 64 KiB.
const PIECE: u64 = 128;
/// The most of `boot/slot` read: the record is a few lines.
const RECORD_MAX: usize = 4096;

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

/// A disk as `diskmap` and `espfat` read it. It never writes.
pub(crate) struct ReadOnly<'a>(pub(crate) &'a mut dyn Disk);

impl diskmap::Disk for ReadOnly<'_> {
    fn sectors(&self) -> u64 {
        self.0.sectors()
    }

    fn read(&mut self, lba: u64, out: &mut [u8]) -> Result<(), diskmap::Error> {
        self.0.read(lba, out).map_err(|_| diskmap::Error::Read)
    }
}

impl espfat::Disk for ReadOnly<'_> {
    fn read(&mut self, lba: u64, out: &mut [u8; S]) -> Result<(), espfat::IoError> {
        self.0.read(lba, out).map_err(|_| espfat::IoError)
    }

    fn write(&mut self, _: u64, _: &[u8; S]) -> Result<(), espfat::IoError> {
        Err(espfat::IoError)
    }

    fn flush(&mut self) -> Result<(), espfat::IoError> {
        Ok(())
    }
}

/// The sectors of the FAT32 volume at `from` on `disk`, checked by espfat's
/// boot-sector parse, at most `most`.
fn volume_sectors(disk: &mut dyn Disk, from: u64, most: u64) -> Result<u64, Error> {
    let mut boot = [0u8; S];
    disk.read(from, &mut boot)?;
    match espfat::Geometry::parse(&boot, from, most) {
        Ok(g) => Ok(u64::from(g.sectors)),
        Err(espfat::Error::NotFat32(why)) => Err(Error::Source(why)),
        Err(_) => Err(Error::Source("a running partition holds no FAT32 volume")),
    }
}

/// The SHA-256 of `sectors` from `lba`.
fn hash(disk: &mut dyn Disk, lba: u64, sectors: u64, piece: &mut [u8]) -> Result<[u8; 32], Error> {
    let mut h = Sha256::new();
    let mut done = 0;
    while done < sectors {
        let n = (sectors - done).min(PIECE);
        let bytes = &mut piece[..n as usize * S];
        disk.read(lba + done, bytes)?;
        h.update(&*bytes);
        done += n;
    }
    Ok(h.finalize().into())
}

/// The `release_seq` of the `kai-slot 1` record at `boot/slot` in the volume
/// at `lba`.
fn release_seq(disk: &mut dyn Disk, lba: u64, most: u64) -> Result<u32, Error> {
    let bad = Error::Verify("the slot copy holds no readable boot/slot record");
    let mut volume = espfat::Volume::open(ReadOnly(disk), lba, most).map_err(|_| bad)?;
    let record: Vec<u8> = volume.read_file("boot/slot", RECORD_MAX).map_err(|_| bad)?;
    let text = core::str::from_utf8(&record).map_err(|_| bad)?;
    let mut lines = text.lines();
    if lines.next() != Some("kai-slot 1") {
        return Err(bad);
    }
    lines
        .find_map(|l| l.strip_prefix("release_seq "))
        .and_then(|n| n.parse().ok())
        .ok_or(bad)
}

/// Writes `plan` to `target`, copying from `source`, the running disk.
/// Before any write, the target must still hold the table the plan was made
/// over, and every volume to copy is checked, so a refusal writes nothing.
pub fn apply(plan: &Plan, source: &mut dyn Disk, target: &mut dyn Disk) -> Result<(), Error> {
    if target.sectors() != plan.sectors || current(target) != plan.wipes {
        return Err(Error::Target);
    }
    let mut piece = vec![0u8; PIECE as usize * S];
    for step in &plan.steps {
        match *step {
            Step::Copy { from, most, .. } => {
                volume_sectors(source, from, most)?;
            }
            // The running slot must be the committed one before anything is
            // written; the copy is checked again once durable.
            Step::Check {
                from,
                most,
                expect:
                    Expect::Slot {
                        sha256,
                        release_seq: seq,
                    },
                ..
            } => {
                let sectors = volume_sectors(source, from, most)?;
                if hash(source, from, sectors, &mut piece)? != sha256 {
                    return Err(Error::Source(
                        "the running slot's SHA-256 is not the committed slot's",
                    ));
                }
                if release_seq(source, from, most).ok() != Some(seq) {
                    return Err(Error::Source(
                        "the running slot's record does not name the committed release",
                    ));
                }
            }
            _ => {}
        }
    }
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
                let sectors = volume_sectors(source, from, most)?;
                let mut done = 0;
                while done < sectors {
                    let n = (sectors - done).min(PIECE);
                    let bytes = &mut piece[..n as usize * S];
                    source.read(from + done, bytes)?;
                    target.write(to + done, bytes)?;
                    done += n;
                }
            }
            Step::Check {
                from,
                to,
                most,
                expect,
            } => {
                let sectors = volume_sectors(source, from, most)?;
                let got = hash(target, to, sectors, &mut piece)?;
                match expect {
                    Expect::Same => {
                        if got != hash(source, from, sectors, &mut piece)? {
                            return Err(Error::Verify("a copy differs from the running disk"));
                        }
                    }
                    Expect::Slot {
                        sha256,
                        release_seq: seq,
                    } => {
                        if got != sha256 {
                            return Err(Error::Verify(
                                "the slot copy's SHA-256 is not the committed slot's",
                            ));
                        }
                        if release_seq(target, to, most)? != seq {
                            return Err(Error::Verify(
                                "the slot copy's record names another release",
                            ));
                        }
                    }
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
                format_data(window, uuid, "kai-data", now)
                    .map_err(|(what, e)| Error::Data(what, e))?;
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
