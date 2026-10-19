//! Tensor infos and the size rule of each tensor type the engine knows.

use crate::Error;
use crate::cursor::Cursor;

/// The most dimensions a tensor may have, as in ggml.
pub const MAX_DIMS: usize = 4;

/// Tensor types with a size rule. Any other type in a file is refused while parsing,
/// so a file the engine cannot run never gets as far as its tensors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TensorType {
    F32,
    F16,
    /// Blocks of 32 values: an f16 scale, then 32 signed bytes.
    Q8_0,
}

impl TensorType {
    pub fn from_id(id: u32) -> Option<Self> {
        match id {
            0 => Some(Self::F32),
            1 => Some(Self::F16),
            8 => Some(Self::Q8_0),
            _ => None,
        }
    }

    pub fn id(self) -> u32 {
        match self {
            Self::F32 => 0,
            Self::F16 => 1,
            Self::Q8_0 => 8,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::F32 => "F32",
            Self::F16 => "F16",
            Self::Q8_0 => "Q8_0",
        }
    }

    /// Values per block; a row (the first dimension) holds whole blocks.
    pub fn block_len(self) -> u64 {
        match self {
            Self::F32 | Self::F16 => 1,
            Self::Q8_0 => 32,
        }
    }

    pub fn block_bytes(self) -> u64 {
        match self {
            Self::F32 => 4,
            Self::F16 => 2,
            Self::Q8_0 => 34,
        }
    }
}

/// One entry of the tensor table. `offset` is relative to the start of the data
/// section and `size` comes from the type's size rule, never from the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TensorInfo<'a> {
    pub name: &'a str,
    dims: [u64; MAX_DIMS],
    n_dims: usize,
    pub ty: TensorType,
    pub offset: u64,
    pub size: u64,
}

impl<'a> TensorInfo<'a> {
    pub(crate) fn read(cursor: &mut Cursor<'a>) -> Result<Self, Error> {
        let name = cursor.string()?;
        let n_dims = cursor.u32()?;
        if n_dims == 0 || n_dims as usize > MAX_DIMS {
            return Err(Error::BadDimCount(n_dims));
        }
        let n_dims = n_dims as usize;
        let mut dims = [1; MAX_DIMS];
        for dim in &mut dims[..n_dims] {
            *dim = cursor.u64()?;
        }
        let type_id = cursor.u32()?;
        let offset = cursor.u64()?;
        let ty = TensorType::from_id(type_id).ok_or(Error::UnknownType(type_id))?;
        let size = byte_size(ty, &dims[..n_dims])?;
        Ok(Self {
            name,
            dims,
            n_dims,
            ty,
            offset,
            size,
        })
    }

    /// The dimensions, innermost (the row length) first, as in the file.
    pub fn dims(&self) -> &[u64] {
        &self.dims[..self.n_dims]
    }

    /// The product of the dimensions; checked to fit when the info was read.
    pub fn elements(&self) -> u64 {
        self.dims().iter().product()
    }
}

/// The bytes a tensor of this type and shape takes.
pub fn byte_size(ty: TensorType, dims: &[u64]) -> Result<u64, Error> {
    let elements = dims
        .iter()
        .try_fold(1u64, |n, &d| n.checked_mul(d))
        .ok_or(Error::DimsOverflow)?;
    let row = dims.first().copied().unwrap_or(1);
    if row % ty.block_len() != 0 {
        return Err(Error::PartialBlock);
    }
    (elements / ty.block_len())
        .checked_mul(ty.block_bytes())
        .ok_or(Error::DimsOverflow)
}
