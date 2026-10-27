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
pub(crate) trait Sink: Sized {
    /// A Q4_K super-block: `a` = sum of sc * (codes . x), `b` = sum of m * (sum
    /// of x's bytes) over its sub-blocks.
    fn q4_k(&mut self, d: f32, dmin: f32, dx: f32, a: i32, b: i32);
    /// A Q6_K super-block: `s` = sum of sc * ((codes - 32) . x).
    fn q6_k(&mut self, d: f32, dx: f32, s: i32);

    /// One super-block of a row against each of several tokens: token t's
    /// [`Sink::q4_k`] with `dx[t]`, `a[t]` and `b[t]`.
    #[inline(always)]
    fn q4_k_each<const N: usize>(
        sinks: &mut [Self; N],
        d: f32,
        dmin: f32,
        dx: &[f32; N],
        a: &[i32; N],
        b: &[i32; N],
    ) {
        for (t, sink) in sinks.iter_mut().enumerate() {
            sink.q4_k(d, dmin, dx[t], a[t], b[t]);
        }
    }

    /// As [`Sink::q4_k_each`], for Q6_K.
    #[inline(always)]
    fn q6_k_each<const N: usize>(sinks: &mut [Self; N], d: f32, dx: &[f32; N], s: &[i32; N]) {
        for (t, sink) in sinks.iter_mut().enumerate() {
            sink.q6_k(d, dx[t], s[t]);
        }
    }
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

    /// Four tokens to an SSE2 register: the same multiplies, converts and adds
    /// as [`Total::q4_k`], lane by lane, so the same bits.
    #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
    #[inline(always)]
    fn q4_k_each<const N: usize>(
        sinks: &mut [Self; N],
        d: f32,
        dmin: f32,
        dx: &[f32; N],
        a: &[i32; N],
        b: &[i32; N],
    ) {
        for (q, sinks) in sinks.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let at = 4 * q;
            sse::q4_k(sinks, d, dmin, &dx[at..], &a[at..], &b[at..]);
        }
        for t in N / 4 * 4..N {
            sinks[t].q4_k(d, dmin, dx[t], a[t], b[t]);
        }
    }

    /// As [`Total::q4_k_each`], for Q6_K.
    #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
    #[inline(always)]
    fn q6_k_each<const N: usize>(sinks: &mut [Self; N], d: f32, dx: &[f32; N], s: &[i32; N]) {
        for (q, sinks) in sinks.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let at = 4 * q;
            sse::q6_k(sinks, d, &dx[at..], &s[at..]);
        }
        for t in N / 4 * 4..N {
            sinks[t].q6_k(d, dx[t], s[t]);
        }
    }
}

/// Four tokens' totals at once for [`Total`]'s batched tails. SSE2 is part of
/// every x86_64 target these are compiled for (`cfg(target_feature = "sse2")`),
/// so no runtime dispatch is needed.
#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
mod sse {
    use super::Total;
    use core::arch::x86_64::*;

    /// `Total::q4_k` for four tokens: the first four of `dx`, `a` and `b`.
    #[inline(always)]
    pub(super) fn q4_k(
        totals: &mut [Total; 4],
        d: f32,
        dmin: f32,
        dx: &[f32],
        a: &[i32],
        b: &[i32],
    ) {
        let (dx, a, b) = (four(dx), four(a), four(b));
        let mut now = totals.each_ref().map(|t| t.0);
        // SAFETY: SSE2 is in the compiled target (see the module); each load and
        // store covers one 16-byte array, and loadu and storeu have no alignment
        // rule.
        unsafe {
            let dx = _mm_loadu_ps(dx.as_ptr());
            let (a, b) = (
                _mm_loadu_si128(a.as_ptr().cast()),
                _mm_loadu_si128(b.as_ptr().cast()),
            );
            let terms = _mm_sub_ps(
                _mm_mul_ps(_mm_mul_ps(_mm_set1_ps(d), dx), _mm_cvtepi32_ps(a)),
                _mm_mul_ps(_mm_mul_ps(_mm_set1_ps(dmin), dx), _mm_cvtepi32_ps(b)),
            );
            _mm_storeu_ps(
                now.as_mut_ptr(),
                _mm_add_ps(_mm_loadu_ps(now.as_ptr()), terms),
            );
        }
        for (t, v) in totals.iter_mut().zip(now) {
            t.0 = v;
        }
    }

    /// `Total::q6_k` for four tokens: the first four of `dx` and `s`.
    #[inline(always)]
    pub(super) fn q6_k(totals: &mut [Total; 4], d: f32, dx: &[f32], s: &[i32]) {
        let (dx, s) = (four(dx), four(s));
        let mut now = totals.each_ref().map(|t| t.0);
        // SAFETY: as in `q4_k`.
        unsafe {
            let dx = _mm_loadu_ps(dx.as_ptr());
            let s = _mm_cvtepi32_ps(_mm_loadu_si128(s.as_ptr().cast()));
            let terms = _mm_mul_ps(_mm_mul_ps(_mm_set1_ps(d), dx), s);
            _mm_storeu_ps(
                now.as_mut_ptr(),
                _mm_add_ps(_mm_loadu_ps(now.as_ptr()), terms),
            );
        }
        for (t, v) in totals.iter_mut().zip(now) {
            t.0 = v;
        }
    }

    fn four<T>(v: &[T]) -> &[T; 4] {
        v[..4].try_into().expect("four values")
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
    let (w, y) = q4_k_fours(tier, w, row, x, y);
    for (w, y) in w.chunks_exact(row).zip(y) {
        let mut total = Total(0.0);
        q4_k_row(tier, w, x, &mut total);
        *y = canonical(total.0);
    }
}

/// The SIMD tiers' rows four at a time ([`crate::kquant_avx2::q4_k_dot4`], the
/// bits [`Total`] gives each); the rows and outputs left for one at a time.
#[cfg_attr(
    not(all(target_arch = "x86_64", target_feature = "sse2")),
    expect(unused_variables, reason = "only the baseline tier is compiled")
)]
fn q4_k_fours<'a, 'w>(
    tier: Tier,
    w: &'w [u8],
    row: usize,
    x: &Q8KVec,
    y: &'a mut [f32],
) -> (&'w [u8], &'a mut [f32]) {
    match tier.0 {
        Kind::Baseline => (w, y),
        #[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
        Kind::Avx2 | Kind::Avx512 | Kind::Avx512Vnni => {
            let done = y.len() / 4 * 4;
            let (fours, rest) = y.split_at_mut(done);
            for (w, y) in w.chunks_exact(4 * row).zip(fours.as_chunks_mut::<4>().0) {
                let rows = core::array::from_fn(|r| &w[r * row..(r + 1) * row]);
                // SAFETY: as in `q4_k_row`.
                let totals = unsafe { crate::kquant_avx2::q4_k_dot4(rows, x) };
                *y = totals.map(canonical);
            }
            (&w[done * row..], rest)
        }
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
