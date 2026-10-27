//! The namespace's items (ADR 0004 §2; P2 task 5): how files, folders, names,
//! data and extended attributes sit in a volume's tree under `(oid, kind, off)`,
//! and their readers. [`crate::ns`] has the operations on them.
//!
//! | Item | Key | Value |
//! |---|---|---|
//! | INODE | `(ino, 1, 0)` | an [`Inode`]: type, mode, links, size, parent, times |
//! | DIRENT | `(folder, 2, name hash)` | the folder's names with that hash, sorted, each with its inode and type |
//! | DATA | `(ino, 3, off)` | a file's bytes from `off`, a multiple of 128 KiB, for up to 128 KiB: inline if they fit an item, else a record |
//! | XATTR | `(ino, 4, name hash)` | the inode's extended attributes with that hash, sorted |
//!
//! - **Names** are UTF-8, 1 to 255 bytes, without `/` or NUL, and never `.` or
//!   `..`. A name's hash is seahash-64 seeded with the filesystem's uuid; names
//!   whose hashes collide share one item, as many as it holds.
//! - **Counts.** A folder's size is how many names it holds, and its links are 2
//!   plus its subfolders, as POSIX counts them. A file's links count its names:
//!   one, since format 1 has no hard-link operation.
//! - Every value starts with a format byte, and a longer inode is a later minor
//!   version's: its known prefix is read.

use crate::Error;
use crate::fs::MAX_RECORD;
use crate::vol::FIRST_OID;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use zerocopy::little_endian::{U16, U32, U64};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

/// The root folder's inode.
pub const ROOT: u64 = FIRST_OID;
/// Item kinds.
pub const K_INODE: u8 = 1;
pub const K_DIRENT: u8 = 2;
pub const K_DATA: u8 = 3;
pub const K_XATTR: u8 = 4;
/// A file's data item covers this much of it: one record's worth.
pub const CHUNK: u64 = MAX_RECORD as u64;
/// The longest name, and the longest attribute name, in bytes.
pub const MAX_NAME: usize = 255;
/// The largest file: 256 TiB, far inside the arithmetic.
pub const MAX_FILE: u64 = 1 << 48;

const ITEM_FMT: u8 = 1;
const FILE_MODE: u16 = 0o644;
const DIR_MODE: u16 = 0o755;
/// A name entry's fixed part: name length, type, inode.
const NAME_HEADER: usize = 1 + 1 + 8;
/// An attribute entry's: name length, value length.
const ATTR_HEADER: usize = 1 + 2;
/// The hash's seeds beside the uuid's two words: "kaifs na", "me hash ".
const SEED_C: u64 = u64::from_le_bytes(*b"kaifs na");
const SEED_D: u64 = u64::from_le_bytes(*b"me hash ");

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileType {
    File,
    Dir,
}

impl FileType {
    pub(crate) fn code(self) -> u8 {
        match self {
            FileType::File => 1,
            FileType::Dir => 2,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(FileType::File),
            2 => Some(FileType::Dir),
            _ => None,
        }
    }
}

/// A file's or folder's inode. Times are nanoseconds since 1970, from
/// [`Fs::clock`]; `mode` holds POSIX permission bits for import and export, which
/// Kernel AI does not enforce (capabilities do, ADR 0001).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inode {
    pub kind: FileType,
    pub mode: u16,
    pub nlink: u32,
    /// A file's bytes; a folder's names.
    pub size: u64,
    /// The folder that names it; the root's own inode for the root.
    pub parent: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub crtime: u64,
    /// The generation that last changed it, its data, names or attributes: what
    /// `restore` and `changes` compare. 0 in an inode written before task 6.
    pub changed: u64,
}

#[derive(FromBytes, IntoBytes, Immutable, KnownLayout, Unaligned)]
#[repr(C)]
struct RawInode {
    fmt: u8,
    kind: u8,
    mode: U16,
    nlink: U32,
    size: U64,
    parent: U64,
    mtime: U64,
    ctime: U64,
    crtime: U64,
    changed: U64,
}

/// Bytes in a stored inode.
pub const INODE_BYTES: usize = 56;
/// Bytes in an inode written before task 6, which had no `changed`.
const INODE_BYTES_V0: usize = 48;
const _: () = assert!(size_of::<RawInode>() == INODE_BYTES);

impl Inode {
    pub(crate) fn new(kind: FileType, parent: u64, now: u64) -> Self {
        let (mode, nlink) = match kind {
            FileType::File => (FILE_MODE, 1),
            FileType::Dir => (DIR_MODE, 2),
        };
        Inode {
            kind,
            mode,
            nlink,
            size: 0,
            parent,
            mtime: now,
            ctime: now,
            crtime: now,
            changed: 0,
        }
    }

    pub fn encode(&self) -> [u8; INODE_BYTES] {
        let raw = RawInode {
            fmt: ITEM_FMT,
            kind: self.kind.code(),
            mode: self.mode.into(),
            nlink: self.nlink.into(),
            size: self.size.into(),
            parent: self.parent.into(),
            mtime: self.mtime.into(),
            ctime: self.ctime.into(),
            crtime: self.crtime.into(),
            changed: self.changed.into(),
        };
        let mut out = [0; INODE_BYTES];
        out.copy_from_slice(raw.as_bytes());
        out
    }

    /// Reads a stored inode. A longer one is a later minor version's: its known
    /// prefix is read. One without `changed`, from before task 6, reads it as 0.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let bad = Error::Corrupt("inode");
        if bytes.len() < INODE_BYTES_V0 {
            return Err(bad);
        }
        let mut full = [0; INODE_BYTES];
        let n = bytes.len().min(INODE_BYTES);
        full[..n].copy_from_slice(&bytes[..n]);
        let (raw, _) = RawInode::read_from_prefix(&full[..]).map_err(|_| bad)?;
        let kind = FileType::from_code(raw.kind).ok_or(bad)?;
        if raw.fmt != ITEM_FMT || raw.parent.get() < FIRST_OID {
            return Err(bad);
        }
        Ok(Inode {
            kind,
            mode: raw.mode.get(),
            nlink: raw.nlink.get(),
            size: raw.size.get(),
            parent: raw.parent.get(),
            mtime: raw.mtime.get(),
            ctime: raw.ctime.get(),
            crtime: raw.crtime.get(),
            changed: raw.changed.get(),
        })
    }
}

/// A name in a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub ino: u64,
    pub kind: FileType,
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name != "."
        && name != ".."
        && !name.bytes().any(|b| b == b'/' || b == 0)
}

pub fn valid_attr_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME && !name.bytes().any(|b| b == 0)
}

/// A name's hash on the filesystem whose uuid is `uuid`, keeping `mask`'s bits.
pub fn hash_name(uuid: &[u8; 16], mask: u64, name: &str) -> u64 {
    let word = |at: usize| {
        let mut word = [0; 8];
        word.copy_from_slice(&uuid[at..at + 8]);
        u64::from_le_bytes(word)
    };
    seahash::hash_seeded(name.as_bytes(), word(0), word(8), SEED_C, SEED_D) & mask
}

pub fn encode_names(entries: &[Entry]) -> Vec<u8> {
    let mut out = vec![ITEM_FMT];
    for e in entries {
        out.push(e.name.len() as u8);
        out.push(e.kind.code());
        out.extend_from_slice(&e.ino.to_le_bytes());
        out.extend_from_slice(e.name.as_bytes());
    }
    out
}

/// A DIRENT item's names: at least one, each valid, sorted and distinct.
pub fn decode_names(bytes: &[u8]) -> Result<Vec<Entry>, Error> {
    let bad = Error::Corrupt("folder names");
    let (&fmt, mut rest) = bytes.split_first().ok_or(bad)?;
    if fmt != ITEM_FMT || rest.is_empty() {
        return Err(bad);
    }
    let mut out: Vec<Entry> = Vec::new();
    while !rest.is_empty() {
        let header = rest.get(..NAME_HEADER).ok_or(bad)?;
        let len = usize::from(header[0]);
        let kind = FileType::from_code(header[1]).ok_or(bad)?;
        let mut word = [0; 8];
        word.copy_from_slice(&header[2..10]);
        let ino = u64::from_le_bytes(word);
        let name = rest.get(NAME_HEADER..NAME_HEADER + len).ok_or(bad)?;
        let name = core::str::from_utf8(name).map_err(|_| bad)?;
        let sorted = out.last().is_none_or(|last| last.name.as_str() < name);
        if !valid_name(name) || !sorted || ino < FIRST_OID {
            return Err(bad);
        }
        out.push(Entry {
            name: name.to_string(),
            ino,
            kind,
        });
        rest = &rest[NAME_HEADER + len..];
    }
    Ok(out)
}

pub fn encode_attrs(attrs: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![ITEM_FMT];
    for (name, value) in attrs {
        out.push(name.len() as u8);
        out.extend_from_slice(&(value.len() as u16).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(value);
    }
    out
}

/// An XATTR item's attributes: at least one, each name valid, sorted and
/// distinct.
pub fn decode_attrs(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let bad = Error::Corrupt("attributes");
    let (&fmt, mut rest) = bytes.split_first().ok_or(bad)?;
    if fmt != ITEM_FMT || rest.is_empty() {
        return Err(bad);
    }
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    while !rest.is_empty() {
        let header = rest.get(..ATTR_HEADER).ok_or(bad)?;
        let name_len = usize::from(header[0]);
        let value_len = usize::from(u16::from_le_bytes([header[1], header[2]]));
        let end = ATTR_HEADER + name_len + value_len;
        let name = rest.get(ATTR_HEADER..ATTR_HEADER + name_len).ok_or(bad)?;
        let name = core::str::from_utf8(name).map_err(|_| bad)?;
        let value = rest.get(ATTR_HEADER + name_len..end).ok_or(bad)?;
        let sorted = out.last().is_none_or(|(last, _)| last.as_str() < name);
        if !valid_attr_name(name) || !sorted {
            return Err(bad);
        }
        out.push((name.to_string(), value.to_vec()));
        rest = &rest[end..];
    }
    Ok(out)
}
