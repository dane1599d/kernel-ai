//! [`crate::attn`]'s kernels in AVX2: VCVTPH2PS widens eight halves of a key or
//! value at a time, once for up to four query heads. A score's eight running
//! sums are one register's lanes; a mixed value's lanes are the out values
//! themselves, kept in registers over every position. Multiplies and adds stay
//! separate (no FMA), so each lane rounds exactly as the baseline's does.

use core::arch::x86_64::*;

/// Most query heads one pass widens a row for.
const BATCH: usize = 4;

/// [`crate::attn::scores`]; `hd` is a multiple of 8 and every slice has been
/// checked against the shapes.
#[target_feature(enable = "avx2,f16c")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn scores(
    q: &[f32],
    hd: usize,
    keys: &[u16],
    stride: usize,
    positions: usize,
    scale: f32,
    scores: &mut [f32],
    context: usize,
) {
    let heads = q.len() / hd;
    let mut h = 0;
    while h < heads {
        let n = (heads - h).min(BATCH);
        let (q, scores) = (&q[h * hd..][..n * hd], &mut scores[h * context..]);
        match n {
            4 => scores_n::<4>(q, hd, keys, stride, positions, scale, scores, context),
            3 => scores_n::<3>(q, hd, keys, stride, positions, scale, scores, context),
            2 => scores_n::<2>(q, hd, keys, stride, positions, scale, scores, context),
            _ => scores_n::<1>(q, hd, keys, stride, positions, scale, scores, context),
        }
        h += n;
    }
}

/// N heads' scores: per position, each head's eight running sums in one
/// register, then the baseline's sum of them.
#[target_feature(enable = "avx2,f16c")]
#[allow(clippy::too_many_arguments)]
fn scores_n<const N: usize>(
    q: &[f32],
    hd: usize,
    keys: &[u16],
    stride: usize,
    positions: usize,
    scale: f32,
    scores: &mut [f32],
    context: usize,
) {
    assert!(q.len() == N * hd && hd.is_multiple_of(8), "scores shape");
    assert!(positions == 0 || (positions - 1) * stride + hd <= keys.len());
    assert!(positions <= context && (N - 1) * context + positions <= scores.len());
    for t in 0..positions {
        let row = &keys[t * stride..][..hd];
        let mut sums = [_mm256_setzero_ps(); N];
        for c in (0..hd).step_by(8) {
            // SAFETY: `c + 8 <= hd`, so 8 halves of `row` (16 bytes) and 8 floats
            // of each head's query; every load is unaligned.
            unsafe {
                let k = _mm256_cvtph_ps(_mm_loadu_si128(row[c..].as_ptr().cast()));
                for (h, sum) in sums.iter_mut().enumerate() {
                    let q = _mm256_loadu_ps(q[h * hd + c..].as_ptr());
                    *sum = _mm256_add_ps(*sum, _mm256_mul_ps(q, k));
                }
            }
        }
        for (h, sum) in sums.into_iter().enumerate() {
            scores[h * context + t] = sum8(sum) * scale;
        }
    }
}

/// `((s0 + s1) + (s2 + s3)) + ((s4 + s5) + (s6 + s7))`, each add with the lower
/// lane first, as the baseline adds them.
#[target_feature(enable = "avx2")]
fn sum8(s: __m256) -> f32 {
    let (lo, hi) = (_mm256_castps256_ps128(s), _mm256_extractf128_ps::<1>(s));
    // s0+s1, s2+s3, s4+s5, s6+s7
    let pairs = _mm_hadd_ps(lo, hi);
    // (s0+s1)+(s2+s3), (s4+s5)+(s6+s7), ...
    let quads = _mm_hadd_ps(pairs, pairs);
    _mm_cvtss_f32(_mm_add_ss(quads, _mm_movehdup_ps(quads)))
}

/// [`crate::attn::mix`]; `hd` is a multiple of 64 and every slice has been
/// checked against the shapes.
#[target_feature(enable = "avx2,f16c")]
pub(crate) fn mix(
    scores: &[f32],
    context: usize,
    values: &[u16],
    stride: usize,
    positions: usize,
    hd: usize,
    out: &mut [f32],
) {
    let heads = out.len() / hd;
    let mut h = 0;
    while h < heads {
        let n = (heads - h).min(BATCH);
        let (scores, out) = (&scores[h * context..], &mut out[h * hd..][..n * hd]);
        // Eight registers of running values: heads times 8-float columns.
        match n {
            4 => mix_n::<4, 2>(scores, context, values, stride, positions, hd, out),
            3 => mix_n::<3, 2>(scores, context, values, stride, positions, hd, out),
            2 => mix_n::<2, 4>(scores, context, values, stride, positions, hd, out),
            _ => mix_n::<1, 8>(scores, context, values, stride, positions, hd, out),
        }
        h += n;
    }
}

/// N heads' mixed values, C columns of 8 at a time: the columns' running values
/// stay in registers while every position's value adds into them in order.
#[target_feature(enable = "avx2,f16c")]
fn mix_n<const N: usize, const C: usize>(
    scores: &[f32],
    context: usize,
    values: &[u16],
    stride: usize,
    positions: usize,
    hd: usize,
    out: &mut [f32],
) {
    assert!(out.len() == N * hd && hd.is_multiple_of(8 * C), "mix shape");
    assert!(positions == 0 || (positions - 1) * stride + hd <= values.len());
    assert!(positions <= context && (N - 1) * context + positions <= scores.len());
    for c in (0..hd).step_by(8 * C) {
        let mut sums = [[_mm256_setzero_ps(); C]; N];
        for t in 0..positions {
            let row = &values[t * stride + c..][..8 * C];
            let mut v = [_mm256_setzero_ps(); C];
            for (j, v) in v.iter_mut().enumerate() {
                // SAFETY: 8 halves (16 bytes) of `row`, unaligned.
                *v = _mm256_cvtph_ps(unsafe { _mm_loadu_si128(row[8 * j..].as_ptr().cast()) });
            }
            for (h, sums) in sums.iter_mut().enumerate() {
                let p = _mm256_set1_ps(scores[h * context + t]);
                for (sum, &v) in sums.iter_mut().zip(&v) {
                    *sum = _mm256_add_ps(*sum, _mm256_mul_ps(p, v));
                }
            }
        }
        for (h, sums) in sums.iter().enumerate() {
            for (j, &sum) in sums.iter().enumerate() {
                let at = &mut out[h * hd + c + 8 * j..][..8];
                // SAFETY: 8 writable floats, unaligned.
                unsafe { _mm256_storeu_ps(at.as_mut_ptr(), sum) };
            }
        }
    }
}
