//! The Q8_0 matrix-vector product, in a baseline tier and an AVX2 tier, which
//! the AVX-512 tiers also run.
//!
//! Both tiers compute each block's dot product as an exact i32 (32 products of
//! at most 128 * 127), then add `(d_w * d_x) * sum` to the row's total block by
//! block, multiplying and adding separately (Rust never fuses them). Only the
//! integer part differs between tiers, so every tier gives the same bits.

use crate::f16;
use crate::kquant::canonical;
use crate::q8_0::{BLOCK_BYTES, BLOCK_LEN, QVec, row_bytes};
use crate::tier::{Kind, Tier};

/// `y[r]` = row `r` of `w` times `x`, for every row. `w` holds `y.len()` rows of
/// Q8_0 blocks, `x.len()` values each. A row that comes to NaN gives the one
/// canonical NaN: which NaN an add of two NaNs keeps is up to the compiler.
pub fn matvec(tier: Tier, w: &[u8], x: &QVec, y: &mut [f32]) {
    let row = row_bytes(x.len());
    assert_eq!(w.len(), row * y.len(), "matvec shape");
    if row == 0 {
        y.fill(0.0);
        return;
    }
    match tier.0 {
        Kind::Baseline => {
            for (w, y) in w.chunks_exact(row).zip(y) {
                *y = canonical(dot_baseline(w, x));
            }
        }
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        Kind::Avx2 | Kind::Avx512 | Kind::Avx512Vnni => {
            for (w, y) in w.chunks_exact(row).zip(y) {
                // SAFETY: every one of these tiers exists only when the CPU has
                // AVX2 and FMA and the OS saves ymm state.
                *y = canonical(unsafe { avx2::dot(w, x) });
            }
        }
    }
}

pub(crate) fn scale(block: &[u8]) -> f32 {
    f16::to_f32(u16::from_le_bytes([block[0], block[1]]))
}

fn dot_baseline(row: &[u8], x: &QVec) -> f32 {
    dot_baseline_n(row, core::array::from_ref(x))[0]
}

/// One row against each of `xs`, as [`dot_baseline`] runs each, with every
/// block's scale widened once for all of them.
pub(crate) fn dot_baseline_n<const N: usize>(row: &[u8], xs: &[QVec; N]) -> [f32; N] {
    let mut totals = [0.0f32; N];
    for (b, w) in row.as_chunks::<BLOCK_BYTES>().0.iter().enumerate() {
        let dw = scale(w);
        for (x, total) in xs.iter().zip(&mut totals) {
            let mut sum = 0i32;
            for (&a, &q) in w[2..].iter().zip(&x.qs.as_chunks::<BLOCK_LEN>().0[b]) {
                sum += i32::from(a as i8) * i32::from(q);
            }
            *total += (dw * x.scales[b]) * sum as f32;
        }
    }
    totals
}

#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
pub(crate) mod avx2 {
    use super::scale;
    use crate::batch_avx2::hsums;
    use crate::kquant::{Sink, Total};
    use crate::q8_0::{BLOCK_BYTES, BLOCK_LEN, QVec};
    use core::arch::x86_64::*;

    /// One row. Per block: |w| as unsigned bytes times x carrying w's sign
    /// (VPSIGNB, which could not negate an x of -128, but x never is), multiplied
    /// in pairs that fit in i16 (VPMADDUBSW, |w| <= 128 and |x| <= 127), widened to
    /// i32 and summed across lanes exactly.
    #[target_feature(enable = "avx2,fma")]
    pub fn dot(row: &[u8], x: &QVec) -> f32 {
        let ones = _mm256_set1_epi16(1);
        let mut total = 0.0f32;
        let blocks = row.as_chunks::<BLOCK_BYTES>().0.iter();
        let xs = x.qs.as_chunks::<BLOCK_LEN>().0;
        for ((w, xq), &dx) in blocks.zip(xs).zip(&x.scales) {
            let qw = &w[2..BLOCK_BYTES];
            // SAFETY: both slices are exactly 32 bytes; loadu has no alignment rule.
            let (wv, xv) = unsafe {
                (
                    _mm256_loadu_si256(qw.as_ptr().cast()),
                    _mm256_loadu_si256(xq.as_ptr().cast()),
                )
            };
            let abs_w = _mm256_sign_epi8(wv, wv);
            let signed_x = _mm256_sign_epi8(xv, wv);
            let pairs = _mm256_maddubs_epi16(abs_w, signed_x);
            let sum = hsum(_mm256_madd_epi16(pairs, ones));
            total += (scale(w) * dx) * sum as f32;
        }
        total
    }

    /// One row against each of `xs`, as [`dot`] runs each: per block, the
    /// codes are loaded, their signs taken and the scale widened once for all
    /// the tokens, eight tokens' lanes are summed by one tree of adds, and four
    /// tokens' float tails `(d_w * d_x) * sum` run in one SSE register (Q6_K's
    /// tail, [`Sink::q6_k_each`], is the same arithmetic).
    #[target_feature(enable = "avx2,fma")]
    pub fn dot_n<const N: usize>(row: &[u8], xs: &[QVec; N]) -> [f32; N] {
        let ones = _mm256_set1_epi16(1);
        let blocks = row.as_chunks::<BLOCK_BYTES>().0;
        // Each token's bytes and scales cut to the row's blocks once, so the
        // block loop indexes them without further bounds checks.
        let qs: [&[[i8; BLOCK_LEN]]; N] =
            core::array::from_fn(|t| &xs[t].qs.as_chunks().0[..blocks.len()]);
        let scales: [&[f32]; N] = core::array::from_fn(|t| &xs[t].scales[..blocks.len()]);
        let mut totals = xs.each_ref().map(|_| Total(0.0));
        for (b, w) in blocks.iter().enumerate() {
            let qw: &[u8; 32] = w[2..].try_into().expect("32 code bytes");
            // SAFETY: 32 readable bytes; loadu has no alignment rule. AVX2 was
            // dispatched.
            let wv = unsafe { _mm256_loadu_si256(qw.as_ptr().cast()) };
            let abs_w = _mm256_sign_epi8(wv, wv);
            let lanes: [__m256i; N] = core::array::from_fn(|t| {
                // SAFETY: as above, 32 bytes.
                let xv = unsafe { _mm256_loadu_si256(qs[t][b].as_ptr().cast()) };
                let pairs = _mm256_maddubs_epi16(abs_w, _mm256_sign_epi8(xv, wv));
                _mm256_madd_epi16(pairs, ones)
            });
            let dx = core::array::from_fn(|t| scales[t][b]);
            Total::q6_k_each(&mut totals, scale(w), &dx, &hsums(&lanes));
        }
        totals.map(|t| t.0)
    }

    #[target_feature(enable = "avx2")]
    pub(crate) fn hsum(v: __m256i) -> i32 {
        let s = _mm_add_epi32(_mm256_castsi256_si128(v), _mm256_extracti128_si256::<1>(v));
        let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b01_00_11_10>(s));
        let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b10_11_00_01>(s));
        _mm_cvtsi128_si32(s)
    }
}
