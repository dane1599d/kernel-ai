//! Q8_0 blocks: 32 values as an f16 scale `d` and 32 signed bytes `q`, value i
//! being `d * q[i]`, 34 bytes in all. A row is a run of blocks, as in GGUF.

use crate::f16;
use alloc::vec;
use alloc::vec::Vec;

pub const BLOCK_LEN: usize = 32;
pub const BLOCK_BYTES: usize = 34;

/// Bytes a row of `len` values takes; `len` is a whole number of blocks.
pub fn row_bytes(len: usize) -> usize {
    assert!(len.is_multiple_of(BLOCK_LEN), "a Q8_0 row of {len} values");
    len / BLOCK_LEN * BLOCK_BYTES
}

/// One block's scale and bytes, quantised as llama.cpp's x86 path does: the scale
/// is max|x| / 127 rounded to f16, and each value is x * (127 / max|x|) rounded to
/// nearest with ties to even. Every byte is in [-127, 127], never -128.
fn quantize_block(x: &[f32], q: &mut [i8]) -> u16 {
    let amax = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let d = f16::from_f32(amax / 127.0);
    let id = if amax != 0.0 { 127.0 / amax } else { 0.0 };
    for (q, &v) in q.iter_mut().zip(x) {
        *q = round_even(v * id).clamp(-127.0, 127.0) as i8;
    }
    d
}

/// Nearest integer, ties to even, for |v| up to 2^22 (NaN stays NaN, and a NaN
/// cast to i8 is 0). Adding and taking away 1.5 * 2^23 leaves no fraction bits.
fn round_even(v: f32) -> f32 {
    const MAGIC: f32 = 12_582_912.0;
    if v.abs() < 4_194_304.0 {
        (v + MAGIC) - MAGIC
    } else {
        v
    }
}

/// Quantises `x` into Q8_0 blocks in GGUF's layout. `out` is `row_bytes(x.len())`.
pub fn quantize_row(x: &[f32], out: &mut [u8]) {
    assert_eq!(out.len(), row_bytes(x.len()));
    let mut q = [0i8; BLOCK_LEN];
    let blocks = out.as_chunks_mut::<BLOCK_BYTES>().0;
    for (x, block) in x.as_chunks::<BLOCK_LEN>().0.iter().zip(blocks) {
        let d = quantize_block(x, &mut q);
        block[..2].copy_from_slice(&d.to_le_bytes());
        for (b, q) in block[2..].iter_mut().zip(q) {
            *b = q as u8;
        }
    }
}

/// Expands Q8_0 blocks into `out`, which holds 32 values per block.
pub fn dequantize_row(blocks: &[u8], out: &mut [f32]) {
    assert_eq!(blocks.len(), row_bytes(out.len()));
    let outs = out.as_chunks_mut::<BLOCK_LEN>().0;
    for (block, out) in blocks.as_chunks::<BLOCK_BYTES>().0.iter().zip(outs) {
        let d = f16::to_f32(u16::from_le_bytes([block[0], block[1]]));
        for (o, &q) in out.iter_mut().zip(&block[2..]) {
            *o = d * f32::from(q as i8);
        }
    }
}

/// An activation vector quantised to Q8_0 for [`crate::matvec`], with its scales
/// widened to f32 once. Its bytes are never -128, which the AVX2 tier relies on.
#[derive(Clone, Debug)]
pub struct QVec {
    pub(crate) qs: Vec<i8>,
    pub(crate) scales: Vec<f32>,
}

impl QVec {
    /// Room for `len` values, a whole number of blocks.
    pub fn new(len: usize) -> Self {
        assert!(
            len.is_multiple_of(BLOCK_LEN),
            "a Q8_0 vector of {len} values"
        );
        Self {
            qs: vec![0; len],
            scales: vec![0.0; len / BLOCK_LEN],
        }
    }

    pub fn len(&self) -> usize {
        self.qs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.qs.is_empty()
    }

    /// Quantises `x`, which has exactly [`QVec::len`] values.
    pub fn quantize(&mut self, x: &[f32]) {
        assert_eq!(x.len(), self.qs.len());
        let qs = self.qs.as_chunks_mut::<BLOCK_LEN>().0;
        let blocks = x.as_chunks::<BLOCK_LEN>().0.iter().zip(qs);
        for ((x, q), scale) in blocks.zip(&mut self.scales) {
            *scale = f16::to_f32(quantize_block(x, q));
        }
    }

    /// The values the quantised vector stands for.
    #[cfg(test)]
    pub(crate) fn dequantize(&self, out: &mut [f32]) {
        assert_eq!(out.len(), self.qs.len());
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.scales[i / BLOCK_LEN] * f32::from(self.qs[i]);
        }
    }
}
