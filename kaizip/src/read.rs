//! An archive opened: every entry checked from the central directory before
//! any is extracted, then extracted with its real bytes counted.

use super::names::{self, Names};
use super::{Error, Limits};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek};
use zip::CompressionMethod;
use zip::ZipArchive;
use zip::result::ZipError;

/// Bytes read from an entry at a time.
const PIECE: usize = 64 * 1024;
/// The type bits of a Unix mode, and the two taken.
const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;
const S_IFDIR: u32 = 0o040_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Folder,
}

/// One entry, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// A relative path of plain parts joined by `/` ([`names::check`]).
    pub path: String,
    pub kind: Kind,
    /// Its size, as its header says; extraction holds it to that.
    pub size: u64,
    /// Its packed size.
    pub packed: u64,
    /// Its place in the archive.
    index: usize,
}

/// Where an archive is extracted to. Each folder is made before anything in
/// it, the folders an entry's name implies included, and each file is
/// `begin`, its bytes, then `end`. An error stops the extraction.
pub trait Unpack {
    fn folder(&mut self, path: &str) -> Result<(), String>;
    fn begin(&mut self, path: &str, size: u64) -> Result<(), String>;
    fn write(&mut self, bytes: &[u8]) -> Result<(), String>;
    fn end(&mut self) -> Result<(), String>;
}

/// An archive's files and folders in memory: for tests, and for a caller
/// that wants an archive's small contents whole.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Tree {
    pub folders: BTreeSet<String>,
    pub files: BTreeMap<String, Vec<u8>>,
    open: Option<String>,
}

impl Unpack for Tree {
    fn folder(&mut self, path: &str) -> Result<(), String> {
        self.folders.insert(path.into());
        Ok(())
    }

    fn begin(&mut self, path: &str, _: u64) -> Result<(), String> {
        self.files.insert(path.into(), Vec::new());
        self.open = Some(path.into());
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        let open = self.open.as_ref().ok_or("no file is open")?;
        self.files
            .get_mut(open)
            .ok_or("no file is open")?
            .extend_from_slice(bytes);
        Ok(())
    }

    fn end(&mut self) -> Result<(), String> {
        self.open = None;
        Ok(())
    }
}

/// An archive whose every entry passed [`Limits`].
pub struct Archive<R: Read + Seek> {
    zip: ZipArchive<R>,
    entries: Vec<Entry>,
    limits: Limits,
}

impl<R: Read + Seek> Archive<R> {
    /// Reads `reader`'s central directory and checks every entry: the whole
    /// archive is refused at the first entry that fails, with why.
    pub fn open(reader: R, limits: Limits) -> Result<Self, Error> {
        let mut zip = ZipArchive::new(reader).map_err(not_zip)?;
        if zip.len() > limits.max_entries {
            return Err(Error::TooManyEntries {
                count: zip.len(),
                max: limits.max_entries,
            });
        }
        if zip.has_duplicate_names() {
            return Err(Error::Duplicate {
                name: String::from("an entry"),
            });
        }
        if zip.has_overlapping_entries().map_err(not_zip)? {
            return Err(Error::Overlap);
        }
        let mut names = Names::default();
        let mut entries = Vec::with_capacity(zip.len());
        let mut total = 0u64;
        for index in 0..zip.len() {
            let data = zip.by_index_data(index).map_err(not_zip)?;
            let raw = String::from_utf8_lossy(data.name_raw()).into_owned();
            let name = data.name().map_err(not_zip)?;
            if data.encrypted() {
                return Err(Error::Encrypted { name: raw });
            }
            let mode = data.unix_mode().map(|m| m & S_IFMT);
            let special =
                data.is_symlink() || mode.is_some_and(|m| m != 0 && m != S_IFREG && m != S_IFDIR);
            if special {
                return Err(Error::Special { name: raw });
            }
            let (path, dir) = names::check(&name, &limits)?;
            let dir = dir || data.is_dir() || mode == Some(S_IFDIR);
            if !matches!(
                data.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            ) {
                return Err(Error::Method {
                    name: path,
                    method: format!("{:?}", data.compression()),
                });
            }
            let (size, packed) = (data.size(), data.compressed_size());
            if size > limits.max_entry {
                return Err(Error::TooLarge {
                    name: path,
                    size,
                    max: limits.max_entry,
                });
            }
            total = total.saturating_add(size);
            if total > limits.max_total {
                return Err(Error::TooLarge {
                    name: String::from("everything in it"),
                    size: total,
                    max: limits.max_total,
                });
            }
            if size > limits.ratio_floor && size / packed.max(1) > limits.max_ratio {
                return Err(Error::Ratio { name: path });
            }
            names.take(&path, dir)?;
            let kind = match dir {
                true => Kind::Folder,
                false => Kind::File,
            };
            entries.push(Entry {
                path,
                kind,
                size,
                packed,
                index,
            });
        }
        Ok(Self {
            zip,
            entries,
            limits,
        })
    }

    /// Every entry, in the archive's order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Extracts every entry to `out`. An entry whose bytes are not what its
    /// header says (its size or its CRC-32) stops it, as does passing
    /// [`Limits::max_total`] in real bytes.
    pub fn extract(&mut self, out: &mut dyn Unpack) -> Result<(), Error> {
        let mut made = BTreeSet::new();
        let mut total = 0u64;
        let mut buf = vec![0u8; PIECE];
        for at in 0..self.entries.len() {
            let entry = self.entries[at].clone();
            let parents = entry.path.match_indices('/').map(|(i, _)| &entry.path[..i]);
            let folders =
                parents.chain((entry.kind == Kind::Folder).then_some(entry.path.as_str()));
            for folder in folders {
                if made.insert(String::from(folder)) {
                    out.folder(folder).map_err(Error::Io)?;
                }
            }
            if entry.kind == Kind::File {
                total = self.extract_file(&entry, out, &mut buf, total)?;
            }
        }
        Ok(())
    }

    /// Extracts one file; `total`, the real bytes so far, grown by it.
    fn extract_file(
        &mut self,
        entry: &Entry,
        out: &mut dyn Unpack,
        buf: &mut [u8],
        mut total: u64,
    ) -> Result<u64, Error> {
        let damaged = |why: String| Error::Damaged {
            name: entry.path.clone(),
            why,
        };
        let mut file = self
            .zip
            .by_index(entry.index)
            .map_err(|e| damaged(e.to_string()))?;
        out.begin(&entry.path, entry.size).map_err(Error::Io)?;
        let mut got = 0u64;
        loop {
            let n = file.read(buf).map_err(|e| damaged(e.to_string()))?;
            if n == 0 {
                break;
            }
            got += n as u64;
            total += n as u64;
            if got > entry.size {
                return Err(damaged(String::from("it holds more than its header says")));
            }
            if total > self.limits.max_total {
                return Err(Error::TooLarge {
                    name: String::from("everything in it"),
                    size: total,
                    max: self.limits.max_total,
                });
            }
            out.write(&buf[..n]).map_err(Error::Io)?;
        }
        if got != entry.size {
            return Err(damaged(String::from("it holds less than its header says")));
        }
        out.end().map_err(Error::Io)?;
        Ok(total)
    }
}

fn not_zip(error: ZipError) -> Error {
    match error {
        ZipError::UnsupportedArchive(why) if why.contains("assword") => Error::Encrypted {
            name: String::from("an entry"),
        },
        error => Error::NotZip(error.to_string()),
    }
}
