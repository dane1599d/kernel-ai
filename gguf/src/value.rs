//! Metadata values. Strings and arrays borrow the file's bytes; an array is walked
//! once when it is read, so iterating it later cannot fail.

use crate::Error;
use crate::cursor::Cursor;

/// A metadata value's type, as numbered in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueType {
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    F32,
    Bool,
    Str,
    Array,
    U64,
    I64,
    F64,
}

impl ValueType {
    const ALL: [Self; 13] = [
        Self::U8,
        Self::I8,
        Self::U16,
        Self::I16,
        Self::U32,
        Self::I32,
        Self::F32,
        Self::Bool,
        Self::Str,
        Self::Array,
        Self::U64,
        Self::I64,
        Self::F64,
    ];

    pub fn from_id(id: u32) -> Result<Self, Error> {
        usize::try_from(id)
            .ok()
            .and_then(|i| Self::ALL.get(i).copied())
            .ok_or(Error::UnknownValueType(id))
    }

    pub fn id(self) -> u32 {
        self as u32
    }

    /// Bytes per value, for every type but strings and arrays.
    fn fixed_size(self) -> Option<usize> {
        match self {
            Self::U8 | Self::I8 | Self::Bool => Some(1),
            Self::U16 | Self::I16 => Some(2),
            Self::U32 | Self::I32 | Self::F32 => Some(4),
            Self::U64 | Self::I64 | Self::F64 => Some(8),
            Self::Str | Self::Array => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value<'a> {
    U8(u8),
    I8(i8),
    U16(u16),
    I16(i16),
    U32(u32),
    I32(i32),
    F32(f32),
    Bool(bool),
    Str(&'a str),
    Array(Array<'a>),
    U64(u64),
    I64(i64),
    F64(f64),
}

impl<'a> Value<'a> {
    pub(crate) fn read(cursor: &mut Cursor<'a>, ty: ValueType) -> Result<Self, Error> {
        Ok(match ty {
            ValueType::U8 => Self::U8(cursor.u8()?),
            ValueType::I8 => Self::I8(cursor.u8()? as i8),
            ValueType::U16 => Self::U16(cursor.u16()?),
            ValueType::I16 => Self::I16(cursor.u16()? as i16),
            ValueType::U32 => Self::U32(cursor.u32()?),
            ValueType::I32 => Self::I32(cursor.u32()? as i32),
            ValueType::F32 => Self::F32(f32::from_bits(cursor.u32()?)),
            ValueType::Bool => Self::Bool(cursor.u8()? != 0),
            ValueType::Str => Self::Str(cursor.string()?),
            ValueType::Array => Self::Array(Array::read(cursor)?),
            ValueType::U64 => Self::U64(cursor.u64()?),
            ValueType::I64 => Self::I64(cursor.u64()? as i64),
            ValueType::F64 => Self::F64(f64::from_bits(cursor.u64()?)),
        })
    }

    /// Any integer value that is not negative.
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Self::U8(v) => Some(v.into()),
            Self::U16(v) => Some(v.into()),
            Self::U32(v) => Some(v.into()),
            Self::U64(v) => Some(v),
            Self::I8(v) => u64::try_from(v).ok(),
            Self::I16(v) => u64::try_from(v).ok(),
            Self::I32(v) => u64::try_from(v).ok(),
            Self::I64(v) => u64::try_from(v).ok(),
            _ => None,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        self.as_u64().and_then(|v| u32::try_from(v).ok())
    }

    /// An f32, or an f64 rounded to f32. Callers check the range.
    pub fn as_f32(&self) -> Option<f32> {
        match *self {
            Self::F32(v) => Some(v),
            Self::F64(v) => Some(v as f32),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Self::Bool(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&'a str> {
        match *self {
            Self::Str(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<Array<'a>> {
        match *self {
            Self::Array(v) => Some(v),
            _ => None,
        }
    }
}

/// An array of one element type. Arrays of arrays are refused.
#[derive(Clone, Copy, PartialEq)]
pub struct Array<'a> {
    ty: ValueType,
    len: usize,
    bytes: &'a [u8],
}

impl<'a> Array<'a> {
    fn read(cursor: &mut Cursor<'a>) -> Result<Self, Error> {
        let ty = ValueType::from_id(cursor.u32()?)?;
        if ty == ValueType::Array {
            return Err(Error::NestedArray);
        }
        let len = cursor.u64()?;
        let start = cursor.pos();
        match ty.fixed_size() {
            Some(size) => {
                let total = len
                    .checked_mul(size as u64)
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or(Error::Truncated)?;
                cursor.take(total)?;
            }
            // Each string takes at least 8 bytes, so a huge count runs out of bytes
            // after at most remaining / 8 rounds.
            None => {
                for _ in 0..len {
                    cursor.string()?;
                }
            }
        }
        // Every element took at least one byte, so the count fits in usize.
        let len = usize::try_from(len).map_err(|_| Error::Truncated)?;
        Ok(Self {
            ty,
            len,
            bytes: cursor.since(start),
        })
    }

    pub fn element_type(&self) -> ValueType {
        self.ty
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The strings of a string array, in order; `None` for any other type.
    pub fn strs(&self) -> Option<Strs<'a>> {
        (self.ty == ValueType::Str).then(|| Strs {
            cursor: Cursor::new(self.bytes),
            left: self.len,
        })
    }

    /// The values of an integer array as i64, a u64 above `i64::MAX` reading as
    /// `i64::MAX`; `None` for any other type.
    pub fn ints(&self) -> Option<Ints<'a>> {
        let size = match self.ty {
            ValueType::U8 | ValueType::I8 => 1,
            ValueType::U16 | ValueType::I16 => 2,
            ValueType::U32 | ValueType::I32 => 4,
            ValueType::U64 | ValueType::I64 => 8,
            _ => return None,
        };
        Some(Ints {
            ty: self.ty,
            chunks: self.bytes.chunks_exact(size),
        })
    }
}

/// The element type and count, never the bytes: the vocabulary is megabytes.
impl core::fmt::Debug for Array<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Array({:?} x {})", self.ty, self.len)
    }
}

pub struct Strs<'a> {
    cursor: Cursor<'a>,
    left: usize,
}

impl<'a> Iterator for Strs<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        self.cursor.string().ok()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.left, Some(self.left))
    }
}

impl ExactSizeIterator for Strs<'_> {}

pub struct Ints<'a> {
    ty: ValueType,
    chunks: core::slice::ChunksExact<'a, u8>,
}

impl Iterator for Ints<'_> {
    type Item = i64;

    fn next(&mut self) -> Option<i64> {
        let c = self.chunks.next()?;
        let mut wide = [0u8; 8];
        wide[..c.len()].copy_from_slice(c);
        let bits = u64::from_le_bytes(wide);
        Some(match self.ty {
            ValueType::I8 => i64::from(bits as u8 as i8),
            ValueType::I16 => i64::from(bits as u16 as i16),
            ValueType::I32 => i64::from(bits as u32 as i32),
            ValueType::I64 => bits as i64,
            _ => i64::try_from(bits).unwrap_or(i64::MAX),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.chunks.size_hint()
    }
}

impl ExactSizeIterator for Ints<'_> {}
