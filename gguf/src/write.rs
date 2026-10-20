//! A GGUF writer for tests and generated fixtures. It writes what it is given, so
//! it can also write files the reader must refuse.

use crate::tensor::TensorType;
use crate::value::ValueType;
use crate::{DEFAULT_ALIGNMENT, MAGIC, VERSION};
use alloc::vec::Vec;

pub struct Writer {
    metadata: Vec<u8>,
    metadata_count: u64,
    infos: Vec<u8>,
    tensor_count: u64,
    data: Vec<u8>,
    alignment: usize,
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

impl Writer {
    pub fn new() -> Self {
        Self {
            metadata: Vec::new(),
            metadata_count: 0,
            infos: Vec::new(),
            tensor_count: 0,
            data: Vec::new(),
            alignment: DEFAULT_ALIGNMENT as usize,
        }
    }

    /// Lays the data out at `alignment` and records it as `general.alignment`.
    /// Set it before adding tensors: each offset uses the alignment of the moment.
    pub fn alignment(&mut self, alignment: u32) -> &mut Self {
        debug_assert_eq!(self.tensor_count, 0, "alignment set after a tensor");
        self.alignment = alignment.max(1) as usize;
        self.u32(crate::ALIGNMENT_KEY, alignment)
    }

    /// A metadata entry with a raw value type id and payload.
    pub fn raw(&mut self, key: &str, type_id: u32, payload: &[u8]) -> &mut Self {
        put_str(&mut self.metadata, key);
        self.metadata.extend_from_slice(&type_id.to_le_bytes());
        self.metadata.extend_from_slice(payload);
        self.metadata_count += 1;
        self
    }

    pub fn u32(&mut self, key: &str, value: u32) -> &mut Self {
        self.raw(key, ValueType::U32.id(), &value.to_le_bytes())
    }

    pub fn f32(&mut self, key: &str, value: f32) -> &mut Self {
        self.raw(key, ValueType::F32.id(), &value.to_le_bytes())
    }

    pub fn bool(&mut self, key: &str, value: bool) -> &mut Self {
        self.raw(key, ValueType::Bool.id(), &[value as u8])
    }

    pub fn string(&mut self, key: &str, value: &str) -> &mut Self {
        let mut payload = Vec::new();
        put_str(&mut payload, value);
        self.raw(key, ValueType::Str.id(), &payload)
    }

    pub fn strings<S: AsRef<str>>(&mut self, key: &str, values: &[S]) -> &mut Self {
        let mut payload = array_head(ValueType::Str, values.len());
        for value in values {
            put_str(&mut payload, value.as_ref());
        }
        self.raw(key, ValueType::Array.id(), &payload)
    }

    pub fn i32s(&mut self, key: &str, values: &[i32]) -> &mut Self {
        let mut payload = array_head(ValueType::I32, values.len());
        for value in values {
            payload.extend_from_slice(&value.to_le_bytes());
        }
        self.raw(key, ValueType::Array.id(), &payload)
    }

    /// A tensor whose data goes at the next aligned offset. `data` is written as
    /// given, whatever the type's size rule says.
    pub fn tensor(&mut self, name: &str, dims: &[u64], ty: TensorType, data: &[u8]) -> &mut Self {
        let offset = self.data.len().next_multiple_of(self.alignment);
        self.data.resize(offset, 0);
        self.data.extend_from_slice(data);
        self.raw_tensor(name, dims, ty.id(), offset as u64)
    }

    /// A tensor info with a raw type id and offset, and no data.
    pub fn raw_tensor(&mut self, name: &str, dims: &[u64], type_id: u32, offset: u64) -> &mut Self {
        put_str(&mut self.infos, name);
        self.infos
            .extend_from_slice(&(dims.len() as u32).to_le_bytes());
        for dim in dims {
            self.infos.extend_from_slice(&dim.to_le_bytes());
        }
        self.infos.extend_from_slice(&type_id.to_le_bytes());
        self.infos.extend_from_slice(&offset.to_le_bytes());
        self.tensor_count += 1;
        self
    }

    pub fn finish(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(64 + self.metadata.len() + self.infos.len() + self.data.len());
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&self.tensor_count.to_le_bytes());
        out.extend_from_slice(&self.metadata_count.to_le_bytes());
        out.extend_from_slice(&self.metadata);
        out.extend_from_slice(&self.infos);
        out.resize(out.len().next_multiple_of(self.alignment), 0);
        out.extend_from_slice(&self.data);
        out
    }
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u64).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn array_head(ty: ValueType, len: usize) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&ty.id().to_le_bytes());
    payload.extend_from_slice(&(len as u64).to_le_bytes());
    payload
}
