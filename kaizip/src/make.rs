//! Making an archive under the rules an opened one keeps: plain names, no
//! duplicates, the entry count and sizes in [`Limits`]. Every file is
//! deflated, dated 1980-01-01 so the same files make the same bytes, with
//! Unix modes 0644 for files and 0755 for folders.

use super::names::{self, Names};
use super::{Error, Limits};
use std::io::{Read, Seek, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

/// Bytes copied into an entry at a time.
const PIECE: usize = 64 * 1024;

pub struct Maker<W: Write + Seek> {
    zip: ZipWriter<W>,
    names: Names,
    limits: Limits,
    count: usize,
    total: u64,
}

impl<W: Write + Seek> Maker<W> {
    pub fn new(out: W, limits: Limits) -> Self {
        Self {
            zip: ZipWriter::new(out),
            names: Names::default(),
            limits,
            count: 0,
            total: 0,
        }
    }

    fn options(&self, mode: u32) -> SimpleFileOptions {
        SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(DateTime::default())
            .unix_permissions(mode)
            .large_file(self.limits.max_entry > u64::from(u32::MAX))
    }

    /// Takes `path` as a file or a folder, checked as an opened archive's
    /// names are.
    /// The path, and whether it is new: a folder named again is not.
    fn take(&mut self, path: &str, dir: bool) -> Result<(String, bool), Error> {
        let (checked, slash) = names::check(path, &self.limits)?;
        if slash && !dir {
            return Err(Error::Name {
                name: path.into(),
                why: "a file's name ends with /",
            });
        }
        let path = checked;
        if self.count == self.limits.max_entries {
            return Err(Error::TooManyEntries {
                count: self.count + 1,
                max: self.limits.max_entries,
            });
        }
        let new = self.names.take(&path, dir)?;
        self.count += usize::from(new);
        Ok((path, new))
    }

    /// A folder, `path`, with or without its trailing `/`.
    pub fn folder(&mut self, path: &str) -> Result<(), Error> {
        let (path, new) = self.take(path.strip_suffix('/').unwrap_or(path), true)?;
        if !new {
            return Ok(());
        }
        let options = self.options(0o755);
        self.zip
            .add_directory(format!("{path}/"), options)
            .map_err(|e| Error::Io(e.to_string()))
    }

    /// A file, `path`, of what `from` reads to its end: at most
    /// [`Limits::max_entry`], and [`Limits::max_total`] with the rest.
    pub fn file(&mut self, path: &str, from: &mut dyn Read) -> Result<(), Error> {
        let (path, _) = self.take(path, false)?;
        let options = self.options(0o644);
        self.zip
            .start_file(path.as_str(), options)
            .map_err(|e| Error::Io(e.to_string()))?;
        let mut buf = vec![0u8; PIECE];
        let mut size = 0u64;
        loop {
            let n = from.read(&mut buf).map_err(|e| Error::Io(e.to_string()))?;
            if n == 0 {
                return Ok(());
            }
            size += n as u64;
            self.total += n as u64;
            if size > self.limits.max_entry {
                return Err(Error::TooLarge {
                    name: path,
                    size,
                    max: self.limits.max_entry,
                });
            }
            if self.total > self.limits.max_total {
                return Err(Error::TooLarge {
                    name: String::from("everything in it"),
                    size: self.total,
                    max: self.limits.max_total,
                });
            }
            self.zip
                .write_all(&buf[..n])
                .map_err(|e| Error::Io(e.to_string()))?;
        }
    }

    /// Writes the central directory; the archive.
    pub fn finish(self) -> Result<W, Error> {
        self.zip.finish().map_err(|e| Error::Io(e.to_string()))
    }
}
