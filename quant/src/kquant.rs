//! The K-quant matrix-vector products, Q4_K and Q6_K weights against
//! [`Q8KVec`] activations, in every tier.
//!
//! Each tier computes a super-block's integer sums exactly and hands them to a
//! [`Sink`]. The products' sink, [`Total`], adds `(d * dx) * sum` (and Q4_K's
//! minimum term) to the row's total super-block by super-block, with separate
//! multiplies and adds (Rust never fuses them). Only the integer part differs
//! between tiers, so every tier gives the same bits; the tests use a sink that
//! records the integers, to compare them before any rounding.

use crate::q8k::Q8KVec;
use crate::tier::{Kind, Tier};
use crate::{q4_k, q6_k};

/// Takes each super-block's exact integers, in order.
pub(crate) trait Sink {
    /// A Q4_K super-block: `a` = sum of sc * (codes . x), `b` = sum of m * (sum
    /// of x's bytes) over its sub-blocks.
    fn q4_k(&mut self, d: f32, dmin: f32, dx: f32, a: i32, b: i32);
    /// A Q6_K super-block: `s` = sum of sc * ((codes - 32) . x).
    fn q6_k(&mut self, d: f32, dx: f32, s: i32);
}

/// A row's total.
pub(crate) struct Total(pub f32);

impl Sink for Total {
    #[inline(always)]
    fn q4_k(&mut self, d: f32, dmin: f32, dx: f32, a: i32, b: i32) {
        self.0 += (d * dx) * a as f32 - (dmin * dx) * b as f32;
    }

    #[inline(always)]
    fn q6_k(&mut self, d: f32, dx: f32, s: i32) {
        self.0 += (d * dx) * s as f32;
    }
}

/// One Q4_K row against `x` in `tier`, its super-blocks handed to `sink`.
pub(crate) fn q4_k_row<S: Sink>(tier: Tier, row: &[u8], x: &Q8KVec, sink: &mut S) {
    match tier.0 {
        Kind::Baseline => q4_k::dot_baseline(row, x, sink),
        // SAFETY: every SIMD tier exists only when the CPU has AVX2, FMA and F16C
        // (cpufeat's CPUID bits) and the OS saves ymm state (XCR0); see
        // `tier::kinds`. The AVX-512 tiers run the AVX2 kernel for Q4_K, which
        // measured faster than 512-bit ones.
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        Kind::Avx2 | Kind::Avx512 | Kind::Avx512Vnni => unsafe {
            crate::kquant_avx2::q4_k_dot(row, x, sink)
        },
    }
}

/// One Q6_K row against `x` in `tier`, its super-blocks handed to `sink`.
pub(crate) fn q6_k_row<S: Sink>(tier: Tier, row: &[u8], x: &Q8KVec, sink: &mut S) {
    match tier.0 {
        Kind::Baseline => q6_k::dot_baseline(row, x, sink),
        // SAFETY: a tier of each kind exists only when the CPU has the features
        // its kernel enables (cpufeat's CPUID bits: AVX2, FMA and F16C; then
        // AVX-512 F, BW and VL; then VNNI) and the OS saves the registers they
        // use (ymm, then zmm and opmask, in XCR0); see `tier::kinds`.
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        Kind::Avx2 => unsafe { crate::kquant_avx2::q6_k_dot(row, x, sink) },
        // SAFETY: as above.
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        Kind::Avx512 => unsafe { crate::kquant_avx512::q6_k_dot(row, x, sink) },
        // SAFETY: as above.
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        Kind::Avx512Vnni => unsafe { crate::kquant_avx512::q6_k_dot_vnni(row, x, sink) },
    }
}

/// `y[r]` = row `r` of the Q4_K matrix `w` times `x`, for every row. `w` holds
/// `y.len()` rows of `x.len()` values. A row that comes to NaN gives the one
/// canonical NaN.
pub fn q4_k_matvec(tier: Tier, w: &[u8], x: &Q8KVec, y: &mut [f32]) {
    let row = q4_k::row_bytes(x.len());
    assert_eq!(Some(w.len()), row.checked_mul(y.len()), "Q4_K matvec shape");
    if row == 0 {
        y.fill(0.0);
        return;
    }
    for (w, y) in w.chunks_exact(row).zip(y) {
        let mut total = Total(0.0);
        q4_k_row(tier, w, x, &mut total);
        *y = canonical(total.0);
    }
}

/// `y[r]` = row `r` of the Q6_K matrix `w` times `x`, for every row, as
/// [`q4_k_matvec`] does for Q4_K.
pub fn q6_k_matvec(tier: Tier, w: &[u8], x: &Q8KVec, y: &mut [f32]) {
    let row = q6_k::row_bytes(x.len());
    assert_eq!(Some(w.len()), row.checked_mul(y.len()), "Q6_K matvec shape");
    if row == 0 {
        y.fill(0.0);
        return;
    }
    for (w, y) in w.chunks_exact(row).zip(y) {
        let mut total = Total(0.0);
        q6_k_row(tier, w, x, &mut total);
        *y = canonical(total.0);
    }
}

/// NaN as one bit pattern: which NaN an add of two NaNs keeps is up to the
/// compiler.
pub(crate) fn canonical(v: f32) -> f32 {
    if v.is_nan() { f32::NAN } else { v }
}
