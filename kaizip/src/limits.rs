//! The ceilings an archive must keep within, and why one was refused.

use core::fmt;

/// What an archive may hold. [`Limits::default`] is Files' choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The most entries, directories included.
    pub max_entries: usize,
    /// The largest entry, uncompressed.
    pub max_entry: u64,
    /// The most bytes all entries hold together, uncompressed.
    pub max_total: u64,
    /// The most an entry may inflate: its size over its compressed size.
    pub max_ratio: u64,
    /// The size up to which an entry may inflate any amount: a few bytes of
    /// deflate can be a page of text.
    pub ratio_floor: u64,
    /// The most parts a name may have.
    pub max_depth: usize,
}

impl Default for Limits {
    /// 10,000 entries; 1 GiB an entry and 4 GiB in all; at most 250 times
    /// inflated past 1 MiB; 32 deep. Text and logs inflate 10 to 100 times;
    /// deflate's most is about 1,032.
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_entry: 1 << 30,
            max_total: 4 << 30,
            max_ratio: 250,
            ratio_floor: 1 << 20,
            max_depth: 32,
        }
    }
}

/// Why an archive was refused, or a make failed. Each says what in words a
/// person may read, with the entry's name where there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not a zip archive, or one too damaged to read.
    NotZip(String),
    /// More entries than [`Limits::max_entries`].
    TooManyEntries { count: usize, max: usize },
    /// An entry, or all of them together, over the size limits.
    TooLarge { name: String, size: u64, max: u64 },
    /// An entry that inflates more than [`Limits::max_ratio`] times.
    Ratio { name: String },
    /// Entries whose data overlap: one compressed block made many files.
    Overlap,
    /// A name that could reach outside the folder, or that is not a plain
    /// name: why, and the name as it was (lossy).
    Name { name: String, why: &'static str },
    /// Two entries with one name, or a file with entries under it.
    Duplicate { name: String },
    /// An encrypted entry.
    Encrypted { name: String },
    /// A symbolic link, device or other entry that is not a file or a folder.
    Special { name: String },
    /// A compression method other than stored and deflate.
    Method { name: String, method: String },
    /// An entry whose bytes are not what its header says: its size or its
    /// CRC-32.
    Damaged { name: String, why: String },
    /// Where it was being extracted to, or made from, failed.
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotZip(why) => write!(f, "this is not a zip file that can be read ({why})"),
            Error::TooManyEntries { count, max } => {
                write!(f, "it holds {count} items, more than the {max} allowed")
            }
            Error::TooLarge { name, size, max } => {
                write!(
                    f,
                    "{name} would be {size} bytes, more than the {max} allowed"
                )
            }
            Error::Ratio { name } => write!(
                f,
                "{name} would grow far larger than it is packed: it may be a zip bomb"
            ),
            Error::Overlap => write!(f, "its items share their packed data: it may be a zip bomb"),
            Error::Name { name, why } => write!(f, "the name {name:?} is refused: {why}"),
            Error::Duplicate { name } => write!(f, "{name} is in it twice"),
            Error::Encrypted { name } => write!(
                f,
                "{name} is encrypted; password-protected zip files are not opened"
            ),
            Error::Special { name } => {
                write!(
                    f,
                    "{name} is a link or a special file, which are not opened"
                )
            }
            Error::Method { name, method } => {
                write!(f, "{name} is packed with {method}, which cannot be read")
            }
            Error::Damaged { name, why } => write!(f, "{name} is damaged: {why}"),
            Error::Io(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for Error {}
