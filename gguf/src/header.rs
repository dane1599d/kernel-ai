//! The header: magic, version, metadata and the tensor table, up to the start of
//! the tensor data.

use crate::cursor::Cursor;
use crate::tensor::TensorInfo;
use crate::value::{Value, ValueType};
use crate::{Error, MAGIC, VERSION};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// The metadata key that sets the data alignment.
pub const ALIGNMENT_KEY: &str = "general.alignment";
pub const DEFAULT_ALIGNMENT: u64 = 32;

/// The most metadata entries and tensors a file may have. Real files have tens
/// and hundreds (Qwen3-0.6B: 28 and 310); the caps keep a hostile header from
/// growing the maps without bound before it fails.
pub const MAX_METADATA: u64 = 1 << 16;
pub const MAX_TENSORS: u64 = 1 << 16;

/// The fewest bytes one tensor info takes: an empty name, one dimension, the type
/// and the offset.
const MIN_TENSOR_INFO: usize = 8 + 4 + 8 + 4 + 8;

/// A parsed header, borrowing the file's bytes.
#[derive(Debug)]
pub struct Header<'a> {
    metadata: BTreeMap<&'a str, Value<'a>>,
    tensors: Vec<TensorInfo<'a>>,
    by_name: BTreeMap<&'a str, usize>,
    alignment: u64,
    table_end: u64,
    data_offset: u64,
}

impl<'a> Header<'a> {
    /// Parses the header from the start of a file. `bytes` may stop anywhere after
    /// the tensor table: tensor data is not looked at, so a range request for the
    /// first few megabytes is enough. [`Header::check_extents`] checks the data.
    /// Bytes that stop too early fail with an error for which [`Error::is_short`]
    /// is true.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        let mut cursor = Cursor::new(bytes);
        if cursor.take(MAGIC.len())? != MAGIC {
            return Err(Error::BadMagic);
        }
        let version = cursor.u32()?;
        if version != VERSION {
            return Err(Error::UnsupportedVersion(version));
        }
        let tensor_count = cursor.u64()?;
        let kv_count = cursor.u64()?;
        if kv_count > MAX_METADATA || tensor_count > MAX_TENSORS {
            return Err(Error::TooMany);
        }
        let mut metadata = BTreeMap::new();
        for _ in 0..kv_count {
            let key = cursor.string()?;
            let ty = ValueType::from_id(cursor.u32()?)?;
            let value = Value::read(&mut cursor, ty)?;
            if metadata.insert(key, value).is_some() {
                return Err(Error::DuplicateKey);
            }
        }
        let alignment = match metadata.get(ALIGNMENT_KEY) {
            None => DEFAULT_ALIGNMENT,
            // The spec asks for a multiple of 8: a power of two of at least 8.
            Some(Value::U32(a)) if a.is_power_of_two() && *a >= 8 => u64::from(*a),
            Some(_) => return Err(Error::BadAlignment),
        };
        if tensor_count > (cursor.remaining() / MIN_TENSOR_INFO) as u64 {
            return Err(Error::Truncated);
        }
        let mut tensors = Vec::new();
        tensors
            .try_reserve_exact(tensor_count as usize)
            .map_err(|_| Error::OutOfMemory)?;
        let mut by_name = BTreeMap::new();
        for index in 0..tensor_count as usize {
            let info = TensorInfo::read(&mut cursor)?;
            if info.offset % alignment != 0 {
                return Err(Error::BadAlignment);
            }
            if by_name.insert(info.name, index).is_some() {
                return Err(Error::DuplicateTensor);
            }
            tensors.push(info);
        }
        let table_end = cursor.pos() as u64;
        let data_offset = table_end
            .checked_next_multiple_of(alignment)
            .ok_or(Error::Truncated)?;
        Ok(Self {
            metadata,
            tensors,
            by_name,
            alignment,
            table_end,
            data_offset,
        })
    }

    /// Checks that every tensor's data lies inside a file of `file_len` bytes.
    pub fn check_extents(&self, file_len: u64) -> Result<(), Error> {
        for info in &self.tensors {
            self.data_range(info, file_len)?;
        }
        Ok(())
    }

    /// The absolute byte range of a tensor's data in a file of `file_len` bytes.
    pub fn data_range(&self, info: &TensorInfo<'_>, file_len: u64) -> Result<(u64, u64), Error> {
        let start = self
            .data_offset
            .checked_add(info.offset)
            .ok_or(Error::TensorPastEnd)?;
        let end = start.checked_add(info.size).ok_or(Error::TensorPastEnd)?;
        if end > file_len {
            return Err(Error::TensorPastEnd);
        }
        Ok((start, end))
    }

    pub fn get(&self, key: &str) -> Option<&Value<'a>> {
        self.metadata.get(key)
    }

    /// Every metadata entry, sorted by key.
    pub fn metadata(&self) -> impl Iterator<Item = (&'a str, &Value<'a>)> {
        self.metadata.iter().map(|(k, v)| (*k, v))
    }

    /// Every tensor, in the order of the file's table.
    pub fn tensors(&self) -> &[TensorInfo<'a>] {
        &self.tensors
    }

    pub fn tensor(&self, name: &str) -> Option<&TensorInfo<'a>> {
        self.by_name.get(name).map(|&i| &self.tensors[i])
    }

    pub fn alignment(&self) -> u64 {
        self.alignment
    }

    /// Where the tensor table ends: the bytes [`Header::parse`] needs.
    pub fn table_end(&self) -> u64 {
        self.table_end
    }

    /// Where the tensor data starts: the end of the table, rounded up to the
    /// alignment.
    pub fn data_offset(&self) -> u64 {
        self.data_offset
    }
}
