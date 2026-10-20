//! Activations as the matrices take them. Each input a layer's matrices share
//! (the normed residual stream, the attention output, the feed-forward hidden
//! state) is quantised once per token: to Q8_0 blocks for Q8_0 matrices and to
//! K-quant super-blocks for Q4_K and Q6_K ones, only in the formats some matrix
//! reading it takes.

use crate::weights::Matrix;
use alloc::vec::Vec;
use quant::{Product, Q8KVec, QVec};

pub(crate) struct Acts {
    q8: Vec<QVec>,
    k8: Vec<Q8KVec>,
}

impl Acts {
    /// Room for `tokens` vectors of `len` values in each format one of `readers`
    /// takes.
    pub(crate) fn new<'a>(
        len: usize,
        tokens: usize,
        readers: impl IntoIterator<Item = &'a Matrix<'a>>,
    ) -> Self {
        let (mut q8, mut k8) = (false, false);
        for m in readers {
            q8 |= m.takes_q8_0();
            k8 |= !m.takes_q8_0();
        }
        let count = |wanted: bool| if wanted { tokens } else { 0 };
        Self {
            q8: (0..count(q8)).map(|_| QVec::new(len)).collect(),
            k8: (0..count(k8)).map(|_| Q8KVec::new(len)).collect(),
        }
    }

    /// An upper bound on the bytes [`Acts::new`] takes for `tokens` vectors of
    /// `len` values in both formats: a Q8_0 block is 32 bytes and an f32 scale
    /// (1.125 bytes a value), a super-block 256 bytes, an f32 scale and sixteen
    /// i16 sums (about 1.14), so 2.5 bytes a value covers both.
    pub(crate) fn bytes(len: usize, tokens: usize) -> usize {
        (len.saturating_mul(5) / 2).saturating_mul(tokens)
    }

    /// Quantises token `t`'s vector `x` into every format kept.
    pub(crate) fn quantize(&mut self, t: usize, x: &[f32]) {
        if let Some(q) = self.q8.get_mut(t) {
            q.quantize(x);
        }
        if let Some(q) = self.k8.get_mut(t) {
            q.quantize(x);
        }
    }

    /// `m` times the first `tokens` vectors.
    pub(crate) fn times<'b>(&'b self, m: &Matrix<'b>, tokens: usize) -> Product<'b> {
        let q8 = &self.q8[..tokens.min(self.q8.len())];
        let k8 = &self.k8[..tokens.min(self.k8.len())];
        m.product(q8, k8)
    }
}
