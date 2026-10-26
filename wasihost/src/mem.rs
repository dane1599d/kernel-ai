//! Bounds-checked access to the guest's linear memory. Every pointer and length a
//! guest passes goes through here; any range that does not fit is `FAULT`, and no
//! arithmetic on guest values can overflow.

use crate::errno::Errno;

/// The guest's linear memory for the length of one call.
pub struct Guest<'a>(pub &'a mut [u8]);

impl Guest<'_> {
    fn range(&self, ptr: u32, len: u32) -> Result<core::ops::Range<usize>, Errno> {
        let start = ptr as usize;
        let end = start.checked_add(len as usize).ok_or(Errno::FAULT)?;
        if end > self.0.len() {
            return Err(Errno::FAULT);
        }
        Ok(start..end)
    }

    pub fn bytes(&self, ptr: u32, len: u32) -> Result<&[u8], Errno> {
        let range = self.range(ptr, len)?;
        Ok(&self.0[range])
    }

    pub fn bytes_mut(&mut self, ptr: u32, len: u32) -> Result<&mut [u8], Errno> {
        let range = self.range(ptr, len)?;
        Ok(&mut self.0[range])
    }

    pub fn put(&mut self, ptr: u32, data: &[u8]) -> Result<(), Errno> {
        let len = u32::try_from(data.len()).map_err(|_| Errno::FAULT)?;
        self.bytes_mut(ptr, len)?.copy_from_slice(data);
        Ok(())
    }

    pub fn put_u32(&mut self, ptr: u32, value: u32) -> Result<(), Errno> {
        self.put(ptr, &value.to_le_bytes())
    }

    pub fn put_u64(&mut self, ptr: u32, value: u64) -> Result<(), Errno> {
        self.put(ptr, &value.to_le_bytes())
    }

    /// The `(ptr, len)` pairs of an iovec array of `count` entries, all checked
    /// against memory before any is used.
    pub fn iovs(&self, iovs: u32, count: u32) -> Result<alloc::vec::Vec<(u32, u32)>, Errno> {
        if count > crate::MAX_IOVS {
            return Err(Errno::INVAL);
        }
        let table = self.bytes(iovs, count.checked_mul(8).ok_or(Errno::FAULT)?)?;
        let mut out = alloc::vec::Vec::with_capacity(count as usize);
        for entry in table.as_chunks::<8>().0 {
            let ptr = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
            let len = u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]);
            self.range(ptr, len)?;
            out.push((ptr, len));
        }
        Ok(out)
    }
}
