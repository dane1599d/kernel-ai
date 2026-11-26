//! Making an archive under the rules an opened one keeps: plain names, no
//! duplicates, the entry count, sizes and inflation in [`Limits`]. Files are
//! deflated, but one that would inflate past [`Limits::max_ratio`] (2 MiB of
//! zeros, say) is stored instead, so [`crate::Archive::open`] takes all that
//! is made. Every entry is dated 1980-01-01, so the same files make the same
//! bytes, with Unix modes 0644 for files and 0755 for folders.
//!
//! A file that fails once its entry is begun (its source cannot be read, or
//! it runs past a size limit) leaves the archive unfinished: every later
//! call, and [`Maker::finish`], says so. Refused names and duplicates fail
//! before anything is written and leave the maker as it was.

use super::names::{self, Names};
use super::{Error, Limits};
use std::io::{self, Read, Seek, SeekFrom, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

/// Bytes copied into an entry at a time.
const PIECE: usize = 64 * 1024;

/// The archive's writer, with where it is: an entry's packed bytes so far are
/// the distance its data has come.
struct Tally<W> {
    inner: W,
    at: u64,
}

impl<W: Write> Write for Tally<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.at += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Seek> Seek for Tally<W> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.at = self.inner.seek(to)?;
        Ok(self.at)
    }
}

pub struct Maker<W: Write + Seek> {
    zip: ZipWriter<Tally<W>>,
    names: Names,
    limits: Limits,
    count: usize,
    total: u64,
    /// Why the archive cannot be finished: a file failed with its entry
    /// begun.
    spent: Option<Error>,
}

fn io_error(e: impl ToString) -> Error {
    Error::Io(e.to_string())
}

impl<W: Write + Seek> Maker<W> {
    pub fn new(out: W, limits: Limits) -> Self {
        Self {
            zip: ZipWriter::new(Tally { inner: out, at: 0 }),
            names: Names::default(),
            limits,
            count: 0,
            total: 0,
            spent: None,
        }
    }

    fn options(&self, mode: u32) -> SimpleFileOptions {
        SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(DateTime::default())
            .unix_permissions(mode)
            .large_file(self.limits.max_entry > u64::from(u32::MAX))
    }

    /// An error if an earlier file left the archive unfinished.
    fn usable(&self) -> Result<(), Error> {
        match &self.spent {
            None => Ok(()),
            Some(why) => Err(Error::Io(format!("the archive was left unfinished: {why}"))),
        }
    }

    /// Runs `write`, a step that begins an entry; its failure spends the
    /// maker.
    fn spending<T>(
        &mut self,
        write: impl FnOnce(&mut Self) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let result = write(self);
        if let Err(why) = &result {
            self.spent = Some(why.clone());
        }
        result
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
        self.usable()?;
        let (path, new) = self.take(path.strip_suffix('/').unwrap_or(path), true)?;
        if !new {
            return Ok(());
        }
        let options = self.options(0o755);
        self.spending(|m| {
            m.zip
                .add_directory(format!("{path}/"), options)
                .map_err(io_error)
        })
    }

    /// A file, `path`, of what `from` reads to its end: at most
    /// [`Limits::max_entry`], and [`Limits::max_total`] with the rest. If it
    /// deflates past [`Limits::max_ratio`], `from` is read again from where it
    /// was, and the file is stored.
    pub fn file<R: Read + Seek>(&mut self, path: &str, from: &mut R) -> Result<(), Error> {
        self.usable()?;
        let (path, _) = self.take(path, false)?;
        self.spending(|m| m.write_file(&path, from))
    }

    fn write_file<R: Read + Seek>(&mut self, path: &str, from: &mut R) -> Result<(), Error> {
        let start = from.stream_position().map_err(io_error)?;
        let deflated = self.options(0o644);
        let (size, packed) = self.copy(path, from, deflated)?;
        // What `Archive::open` refuses, with `packed` at most the entry's
        // packed size, so this never takes less inflation for more.
        let floor = self.limits.ratio_floor;
        if size <= floor || size / packed.max(1) <= self.limits.max_ratio {
            return Ok(());
        }
        // The entry is the last one written, so it goes, and its bytes are
        // written over: stored, it is longer.
        self.zip.abort_file().map_err(io_error)?;
        self.total -= size;
        from.seek(SeekFrom::Start(start)).map_err(io_error)?;
        let stored = deflated.compression_method(CompressionMethod::Stored);
        self.copy(path, from, stored)?;
        Ok(())
    }

    /// Where the archive's writer is.
    fn at(&self) -> u64 {
        self.zip.get_ref().map_or(0, |tally| tally.at)
    }

    /// Writes an entry of what `from` reads; its size, and, past
    /// [`Limits::ratio_floor`], its packed bytes so far once the deflater has
    /// let go of what it holds (at most the packed size; else 0).
    fn copy(
        &mut self,
        path: &str,
        from: &mut dyn Read,
        options: SimpleFileOptions,
    ) -> Result<(u64, u64), Error> {
        self.zip.start_file(path, options).map_err(io_error)?;
        let data = self.at();
        let mut buf = vec![0u8; PIECE];
        let mut size = 0u64;
        loop {
            let n = from.read(&mut buf).map_err(io_error)?;
            if n == 0 {
                break;
            }
            size += n as u64;
            self.total += n as u64;
            if size > self.limits.max_entry {
                return Err(Error::TooLarge {
                    name: path.into(),
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
            self.zip.write_all(&buf[..n]).map_err(io_error)?;
        }
        if size <= self.limits.ratio_floor {
            return Ok((size, 0));
        }
        // A sync flush: the deflater writes out all it holds, so the
        // distance is the packed size but for the stream's last few bytes.
        self.zip.flush().map_err(io_error)?;
        Ok((size, self.at().saturating_sub(data)))
    }

    /// Writes the central directory; the archive. An error if a file left
    /// it unfinished.
    pub fn finish(self) -> Result<W, Error> {
        self.usable()?;
        let tally = self.zip.finish().map_err(io_error)?;
        Ok(tally.inner)
    }
}
