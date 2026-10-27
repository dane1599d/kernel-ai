//! The stub's one file: records of 128 KiB at [`stub::FILE_BASE`] in `kai-data`,
//! each written with one PART_WRITE once its last byte arrives and checksummed
//! with seahash-64 as kaifs would, and verified when read back from the disk.

use alloc::vec;
use alloc::vec::Vec;
use kabi::handle::Handle;
use kabi::part::{SECTOR, stub};
use user::sys;

const RECORD: usize = stub::RECORD as usize;
const RECORDS: usize = (stub::FILE_MAX / stub::RECORD) as usize;

pub struct File {
    part: Handle,
    /// The record being written or last read, and its number.
    buffer: Vec<u8>,
    held: Option<usize>,
    /// Whether `buffer` holds bytes not yet on the disk.
    dirty: bool,
    /// Each record's checksum once it is on the disk.
    sums: Vec<Option<u64>>,
}

impl File {
    pub fn new(part: Handle, sectors: u64) -> File {
        let fits = (sectors * SECTOR).saturating_sub(stub::FILE_BASE) / stub::RECORD;
        File {
            part,
            buffer: vec![0; RECORD],
            held: None,
            dirty: false,
            sums: vec![None; RECORDS.min(fits as usize)],
        }
    }

    fn sector(record: usize) -> u64 {
        (stub::FILE_BASE + (record * RECORD) as u64) / SECTOR
    }

    /// Makes `record` the held one, writing out a dirty one first; `load` reads
    /// it from the disk and verifies it.
    fn hold(&mut self, record: usize, load: bool) -> Result<(), &'static str> {
        if self.held == Some(record) {
            return Ok(());
        }
        self.write_out()?;
        if record >= self.sums.len() {
            return Err("past the end of the file");
        }
        if load {
            sys::part_read(self.part, Self::sector(record), &mut self.buffer)
                .map_err(|_| "PART_READ failed")?;
            if let Some(sum) = self.sums[record]
                && seahash::hash(&self.buffer) != sum
            {
                return Err("a record failed its checksum");
            }
        } else {
            self.buffer.fill(0);
        }
        self.held = Some(record);
        Ok(())
    }

    /// Writes the held record to the disk if it changed.
    fn write_out(&mut self) -> Result<(), &'static str> {
        let Some(record) = self.held.filter(|_| self.dirty) else {
            return Ok(());
        };
        self.sums[record] = Some(seahash::hash(&self.buffer));
        sys::part_write(self.part, Self::sector(record), &self.buffer)
            .map_err(|_| "PART_WRITE failed")?;
        self.dirty = false;
        Ok(())
    }

    pub fn write(&mut self, offset: u64, mut bytes: &[u8]) -> Result<(), &'static str> {
        let mut at = usize::try_from(offset).map_err(|_| "past the end of the file")?;
        while !bytes.is_empty() {
            let (record, inside) = (at / RECORD, at % RECORD);
            // A record written from its start need not be read first.
            self.hold(
                record,
                inside != 0 && self.sums.get(record).is_some_and(Option::is_some),
            )?;
            let n = bytes.len().min(RECORD - inside);
            self.buffer[inside..inside + n].copy_from_slice(&bytes[..n]);
            self.dirty = true;
            if inside + n == RECORD {
                self.write_out()?;
            }
            (at, bytes) = (at + n, &bytes[n..]);
        }
        Ok(())
    }

    pub fn read(&mut self, offset: u64, out: &mut [u8]) -> Result<(), &'static str> {
        let mut at = usize::try_from(offset).map_err(|_| "past the end of the file")?;
        let mut done = 0;
        while done < out.len() {
            let (record, inside) = (at / RECORD, at % RECORD);
            self.hold(record, true)?;
            let n = (out.len() - done).min(RECORD - inside);
            out[done..done + n].copy_from_slice(&self.buffer[inside..inside + n]);
            (at, done) = (at + n, done + n);
        }
        Ok(())
    }

    /// Writes out the held record, flushes, and forgets it, so reads come from
    /// the disk.
    pub fn flush(&mut self) -> Result<(), &'static str> {
        self.write_out()?;
        sys::part_flush(self.part).map_err(|_| "PART_FLUSH failed")?;
        self.held = None;
        Ok(())
    }
}
