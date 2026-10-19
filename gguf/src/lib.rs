//! A zero-copy reader for GGUF v3 model files, which treats every file as hostile.
//!
//! [`Header::parse`] reads the metadata and the tensor table from the start of a
//! file, which is all `cargo xtask model pin` fetches. [`Gguf::parse`] also checks
//! that every tensor lies inside the whole file and hands out borrowed slices of
//! its data. Offsets and sizes use checked arithmetic, counts are bounded by the
//! bytes left before anything is allocated, and a tensor's size comes from its type
//! and shape, never from the file. A bad file fails with a named [`Error`]; it can
//! never make the reader panic or read out of bounds.
//!
//! Layout, all little-endian: magic `GGUF`, version u32 (3), tensor count u64,
//! metadata count u64; each metadata entry is a key string, a value type u32 and
//! the value; each tensor info is a name string, a dimension count u32, the
//! dimensions u64 (innermost first), a type u32 and an offset u64 into the data;
//! then padding to the alignment (32, or `general.alignment`) and the tensor data.
//! A string is a u64 length and that many bytes of UTF-8.

#![no_std]

extern crate alloc;

mod cursor;
mod header;
mod tensor;
mod value;
pub mod write;

#[cfg(test)]
mod tests;

pub use header::{ALIGNMENT_KEY, DEFAULT_ALIGNMENT, Header, MAX_METADATA, MAX_TENSORS};
pub use tensor::{MAX_DIMS, TensorInfo, TensorType, byte_size};
pub use value::{Array, Ints, Strs, Value, ValueType};

pub const MAGIC: [u8; 4] = *b"GGUF";
pub const VERSION: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The bytes end inside a fixed-size field, or a count promises more than is left.
    Truncated,
    BadMagic,
    UnsupportedVersion(u32),
    /// A string's length runs past the end of the bytes.
    StringPastEnd,
    BadUtf8,
    UnknownValueType(u32),
    NestedArray,
    DuplicateKey,
    /// `general.alignment` is not a power-of-two u32, or a tensor's offset is not a
    /// multiple of the alignment.
    BadAlignment,
    /// A tensor has no dimensions or more than [`MAX_DIMS`].
    BadDimCount(u32),
    /// A tensor's element count or byte size does not fit in a u64.
    DimsOverflow,
    /// A tensor's row is not a whole number of its type's blocks.
    PartialBlock,
    /// A tensor type with no size rule here, so the engine cannot run it.
    UnknownType(u32),
    DuplicateTensor,
    /// A tensor's data runs past the end of the file.
    TensorPastEnd,
    /// More metadata entries or tensors than [`MAX_METADATA`] or [`MAX_TENSORS`].
    TooMany,
    OutOfMemory,
}

impl Error {
    /// The bytes ended too early: a whole file is malformed, but a prefix of one
    /// may just need more bytes.
    pub fn is_short(&self) -> bool {
        matches!(self, Self::Truncated | Self::StringPastEnd)
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::Truncated => f.write_str("the file ends inside its header"),
            Self::BadMagic => f.write_str("not a GGUF file"),
            Self::UnsupportedVersion(v) => write!(f, "GGUF version {v}, expected {VERSION}"),
            Self::StringPastEnd => f.write_str("a string runs past the end of the file"),
            Self::BadUtf8 => f.write_str("a string is not UTF-8"),
            Self::UnknownValueType(t) => write!(f, "unknown metadata value type {t}"),
            Self::NestedArray => f.write_str("an array of arrays"),
            Self::DuplicateKey => f.write_str("a metadata key appears twice"),
            Self::BadAlignment => f.write_str("bad alignment"),
            Self::BadDimCount(n) => write!(f, "a tensor has {n} dimensions"),
            Self::DimsOverflow => f.write_str("a tensor's size overflows"),
            Self::PartialBlock => f.write_str("a tensor row is not whole blocks"),
            Self::UnknownType(t) => write!(
                f,
                "tensor type {t} ({}), which the engine cannot run",
                type_name(t)
            ),
            Self::DuplicateTensor => f.write_str("a tensor name appears twice"),
            Self::TensorPastEnd => f.write_str("a tensor runs past the end of the file"),
            Self::TooMany => f.write_str("too many metadata entries or tensors"),
            Self::OutOfMemory => f.write_str("out of memory"),
        }
    }
}

/// ggml's name for a tensor type id, for messages about types the engine refuses.
/// Ids 4, 5, 31-33 and 36-38 belonged to formats ggml has since removed.
pub fn type_name(id: u32) -> &'static str {
    const NAMES: [&str; 40] = [
        "F32", "F16", "Q4_0", "Q4_1", "removed", "removed", "Q5_0", "Q5_1", "Q8_0", "Q8_1", "Q2_K",
        "Q3_K", "Q4_K", "Q5_K", "Q6_K", "Q8_K", "IQ2_XXS", "IQ2_XS", "IQ3_XXS", "IQ1_S", "IQ4_NL",
        "IQ3_S", "IQ2_S", "IQ4_XS", "I8", "I16", "I32", "I64", "F64", "IQ1_M", "BF16", "removed",
        "removed", "removed", "TQ1_0", "TQ2_0", "removed", "removed", "removed", "MXFP4",
    ];
    NAMES.get(id as usize).copied().unwrap_or("unknown")
}

/// A whole file: its header, with every tensor checked to lie inside the bytes.
pub struct Gguf<'a> {
    header: Header<'a>,
    bytes: &'a [u8],
}

/// Counts only, never the file's bytes, which run to hundreds of megabytes.
impl core::fmt::Debug for Gguf<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Gguf")
            .field("bytes", &self.bytes.len())
            .field("metadata", &self.header.metadata().count())
            .field("tensors", &self.header.tensors().len())
            .finish()
    }
}

/// A tensor's info and its data. The data is only as aligned as the file's offsets
/// and the slice's base make it, so read numbers from it byte by byte.
#[derive(Clone, Copy)]
pub struct Tensor<'a> {
    pub info: TensorInfo<'a>,
    pub data: &'a [u8],
}

impl<'a> Gguf<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        let header = Header::parse(bytes)?;
        header.check_extents(bytes.len() as u64)?;
        Ok(Self { header, bytes })
    }

    pub fn header(&self) -> &Header<'a> {
        &self.header
    }

    pub fn get(&self, key: &str) -> Option<&Value<'a>> {
        self.header.get(key)
    }

    pub fn tensor(&self, name: &str) -> Option<Tensor<'a>> {
        let info = *self.header.tensor(name)?;
        let (start, end) = self
            .header
            .data_range(&info, self.bytes.len() as u64)
            .ok()?;
        let data = self.bytes.get(start as usize..end as usize)?;
        Some(Tensor { info, data })
    }
}
