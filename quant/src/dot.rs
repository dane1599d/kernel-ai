//! The Q8_0 matrix-vector product, in a baseline tier and an AVX2 tier.
//!
//! Both tiers compute each block's dot product as an exact i32 (32 products of
//! at most 128 * 127), then add `(d_w * d_x) * sum` to the row's total block by
//! block, multiplying and adding separately (Rust never fuses them). Only the
//! integer part differs between tiers, so every tier gives the same bits.

use crate::f16;
use crate::q8_0::{BLOCK_BYTES, BLOCK_LEN, QVec, row_bytes};

/// A matvec implementation this CPU can run. Only [`Tier::BASELINE`] and what
/// [`Tier::avx2`] and [`Tier::best`] detect exist, so holding a tier proves the
/// CPU runs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tier(Kind);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Baseline,
    #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
    Avx2,
}

impl Tier {
    /// Plain Rust, for any CPU.
    pub const BASELINE: Self = Self(Kind::Baseline);

    /// The fastest tier this CPU runs.
    pub fn best() -> Self {
        Self::avx2().unwrap_or(Self::BASELINE)
    }

    /// AVX2 and FMA, when the CPU has them and the OS saves ymm state. Targets
    /// without SSE (the soft-float kernel target) never get it.
    pub(crate) fn avx2() -> Option<Self> {
        if !cpufeat::Features::detect().avx2_fma() {
            return None;
        }
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        return Some(Self(Kind::Avx2));
        #[cfg(not(all(target_arch = "x86_64", target_feature = "sse2")))]
        None
    }

    /// Every tier this CPU runs, the baseline first.
    pub fn all() -> impl Iterator<Item = Self> {
        core::iter::once(Self::BASELINE).chain(Self::avx2())
    }

    pub fn name(self) -> &'static str {
        match self.0 {
            Kind::Baseline => "baseline",
            #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
            Kind::Avx2 => "avx2+fma",
        }
    }
}

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
        Kind::Avx2 => {
            for (w, y) in w.chunks_exact(row).zip(y) {
                // SAFETY: a Kind::Avx2 tier exists only when the CPU has AVX2
                // and FMA and the OS saves ymm state.
                *y = canonical(unsafe { avx2::dot(w, x) });
            }
        }
    }
}

fn canonical(v: f32) -> f32 {
    if v.is_nan() { f32::NAN } else { v }
}

fn scale(block: &[u8]) -> f32 {
    f16::to_f32(u16::from_le_bytes([block[0], block[1]]))
}

fn dot_baseline(row: &[u8], x: &QVec) -> f32 {
    let mut total = 0.0f32;
    let blocks = row.as_chunks::<BLOCK_BYTES>().0.iter();
    let xs = x.qs.as_chunks::<BLOCK_LEN>().0;
    for ((w, xq), &dx) in blocks.zip(xs).zip(&x.scales) {
        let mut sum = 0i32;
        for (&a, &b) in w[2..].iter().zip(xq) {
            sum += i32::from(a as i8) * i32::from(b);
        }
        total += (scale(w) * dx) * sum as f32;
    }
    total
}

#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
mod avx2 {
    use super::scale;
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

    #[target_feature(enable = "avx2")]
    fn hsum(v: __m256i) -> i32 {
        let s = _mm_add_epi32(_mm256_castsi256_si128(v), _mm256_extracti128_si256::<1>(v));
        let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b01_00_11_10>(s));
        let s = _mm_add_epi32(s, _mm_shuffle_epi32::<0b10_11_00_01>(s));
        _mm_cvtsi128_si32(s)
    }
}
