//! Activations for the K-quant products: a vector quantised in super-blocks of
//! 256 values, each an f32 scale and 256 signed bytes, with the sum of every 16
//! bytes kept beside them. Q4_K's minimums and Q6_K's offset of 32 multiply those
//! sums, so a super-block's dot product stays one exact integer.

use alloc::vec;
use alloc::vec::Vec;

/// Values per super-block, in the activations and in Q4_K and Q6_K weights.
pub const SUPER_LEN: usize = 256;
/// Values per kept sum: Q6_K's sub-block. Q4_K's sub-blocks are two of them.
pub const SUM_LEN: usize = 16;

/// An activation vector quantised for the K-quant products,
/// [`crate::q4_k_matvec`] and [`crate::q6_k_matvec`]. Its bytes are in
/// [-127, 127], never -128.
#[derive(Clone, Debug)]
pub struct Q8KVec {
    pub(crate) qs: Vec<i8>,
    pub(crate) scales: Vec<f32>,
    pub(crate) sums: Vec<i16>,
}

impl Q8KVec {
    /// Room for `len` values, a whole number of super-blocks.
    pub fn new(len: usize) -> Self {
        assert!(
            len.is_multiple_of(SUPER_LEN),
            "a K-quant vector of {len} values"
        );
        Self {
            qs: vec![0; len],
            scales: vec![0.0; len / SUPER_LEN],
            sums: vec![0; len / SUM_LEN],
        }
    }

    pub fn len(&self) -> usize {
        self.qs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.qs.is_empty()
    }

    /// Quantises `x`, which has exactly [`Q8KVec::len`] values. Per super-block,
    /// as ggml's reference does, each byte is x * (127 / max|x|) rounded to
    /// nearest (ties to even) and the scale is 1 / (127 / max|x|); an all-zero
    /// block gets a zero scale. NaN becomes 0; a block with an infinity gets an
    /// infinite scale and zero bytes, so its products come out NaN or infinite.
    pub fn quantize(&mut self, x: &[f32]) {
        assert_eq!(x.len(), self.qs.len());
        let qs = self.qs.as_chunks_mut::<SUPER_LEN>().0;
        let sums = self.sums.as_chunks_mut::<{ SUPER_LEN / SUM_LEN }>().0;
        let blocks = x.as_chunks::<SUPER_LEN>().0.iter().zip(qs).zip(sums);
        for (((x, q), sums), scale) in blocks.zip(&mut self.scales) {
            let amax = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let id = if amax != 0.0 { 127.0 / amax } else { 0.0 };
            for (q, &v) in q.iter_mut().zip(x) {
                *q = crate::q8_0::round_even(v * id).clamp(-127.0, 127.0) as i8;
            }
            *scale = if amax != 0.0 { 1.0 / id } else { 0.0 };
            for (sum, q) in sums.iter_mut().zip(q.as_chunks::<SUM_LEN>().0) {
                *sum = q.iter().map(|&v| i16::from(v)).sum();
            }
        }
    }

    /// The values the quantised vector stands for.
    #[cfg(test)]
    pub(crate) fn dequantize(&self, out: &mut [f32]) {
        assert_eq!(out.len(), self.qs.len());
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.scales[i / SUPER_LEN] * f32::from(self.qs[i]);
        }
    }
}
