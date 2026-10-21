//! The K-quant row products in AVX2: codes are unsigned (4 or 6 bits), so
//! VPMADDUBSW multiplies them with x's signed bytes directly, in i16 pairs
//! (at most 2 * 63 * 127 = 16,002), and VPMADDWD multiplies each pair by its
//! sub-block's scale into i32 lanes. A super-block's lanes sum to the same exact
//! integers the baseline computes; the lane bounds are in each kernel.

use crate::kquant::Sink;
use crate::q8k::{Q8KVec, SUPER_LEN};
use crate::{q4_k, q6_k};
use core::arch::x86_64::*;

/// One Q4_K row against `x`. Per super-block, each 32-byte run of codes gives
/// sub-block 2g (low nibbles) and 2g+1 (high nibbles). A lane of `acc` adds 8
/// products of at most 2 * 15 * 127 * 63, and the minimum term's lanes
/// 2 * 63 * 2,032: far from overflow.
#[target_feature(enable = "avx2,fma,f16c")]
pub fn q4_k_dot(row: &[u8], x: &Q8KVec, sink: &mut impl Sink) {
    let nibble = _mm256_set1_epi8(15);
    let blocks = row.as_chunks::<{ q4_k::BLOCK_BYTES }>().0.iter();
    let xs = x.qs.as_chunks::<SUPER_LEN>().0;
    let sums = x.sums.as_chunks::<16>().0;
    for (((block, xq), sums), &dx) in blocks.zip(xs).zip(sums).zip(&x.scales) {
        let [d, dmin] = halves(block[..4].try_into().unwrap());
        let (sc, m) = q4_k::packed_scales(block[4..q4_k::CODES].try_into().unwrap());
        // The scales as i16 in both 128-bit lanes, and the minimums each twice.
        let sc = _mm256_broadcastsi128_si256(_mm_cvtepu8_epi16(_mm_cvtsi64_si128(sc as i64)));
        let m = _mm_cvtsi64_si128(m as i64);
        let m = _mm256_cvtepu8_epi16(_mm_unpacklo_epi8(m, m));
        let runs = block[q4_k::CODES..].as_chunks::<32>().0;
        let xq = xq.as_chunks::<32>().0;
        let mut acc = _mm256_setzero_si256();
        for (g, run) in runs.iter().enumerate() {
            let q = load(run);
            let lo = _mm256_and_si256(q, nibble);
            let hi = _mm256_and_si256(_mm256_srli_epi16::<4>(q), nibble);
            let lo = _mm256_madd_epi16(
                _mm256_maddubs_epi16(lo, load_i8(&xq[2 * g])),
                _mm256_shuffle_epi8(sc, lane_of(2 * g, 2 * g)),
            );
            let hi = _mm256_madd_epi16(
                _mm256_maddubs_epi16(hi, load_i8(&xq[2 * g + 1])),
                _mm256_shuffle_epi8(sc, lane_of(2 * g + 1, 2 * g + 1)),
            );
            acc = _mm256_add_epi32(acc, _mm256_add_epi32(lo, hi));
        }
        let mins = _mm256_madd_epi16(m, load_sums(sums));
        let (a, b) = hsum2(acc, mins);
        sink.q4_k(d, dmin, dx, a, b);
    }
}

/// One Q6_K row against `x`. Per half of a super-block, quarter k's codes are a
/// nibble of `ql` and a bit pair of `qh`, put together in place; each quarter
/// spans two sub-blocks of 16, so its scale vector holds two scales. A lane of
/// `acc` adds 8 products of at most 2 * 16,002 * 128; the offset term's lanes
/// 2 * 128 * 2,032.
#[target_feature(enable = "avx2,fma,f16c")]
pub fn q6_k_dot(row: &[u8], x: &Q8KVec, sink: &mut impl Sink) {
    let nibble = _mm256_set1_epi8(15);
    let blocks = row.as_chunks::<{ q6_k::BLOCK_BYTES }>().0.iter();
    let xs = x.qs.as_chunks::<SUPER_LEN>().0;
    let sums = x.sums.as_chunks::<16>().0;
    for (((block, xq), sums), &dx) in blocks.zip(xs).zip(sums).zip(&x.scales) {
        let d = half(u16::from_le_bytes([block[q6_k::D], block[q6_k::D + 1]]));
        let scales = load128(block[q6_k::SCALES..q6_k::D].try_into().unwrap());
        let scales = _mm256_cvtepi8_epi16(scales);
        let low = block[..q6_k::HIGH].as_chunks::<32>().0;
        let high = block[q6_k::HIGH..q6_k::SCALES].as_chunks::<32>().0;
        let xq = xq.as_chunks::<32>().0;
        let mut acc = _mm256_setzero_si256();
        for n in 0..2 {
            let (l0, l1, h) = (load(&low[2 * n]), load(&low[2 * n + 1]), load(&high[n]));
            // Half n's eight scales as i16, in both 128-bit lanes.
            let sc = if n == 0 {
                _mm256_permute2x128_si256::<0x00>(scales, scales)
            } else {
                _mm256_permute2x128_si256::<0x11>(scales, scales)
            };
            // Each shift moves 16-bit lanes, but the masks keep every bit
            // inside its own byte.
            let codes = [
                _mm256_or_si256(
                    _mm256_and_si256(l0, nibble),
                    _mm256_slli_epi16::<4>(_mm256_and_si256(h, _mm256_set1_epi8(0x03))),
                ),
                _mm256_or_si256(
                    _mm256_and_si256(l1, nibble),
                    _mm256_slli_epi16::<2>(_mm256_and_si256(h, _mm256_set1_epi8(0x0c))),
                ),
                _mm256_or_si256(
                    _mm256_and_si256(_mm256_srli_epi16::<4>(l0), nibble),
                    _mm256_and_si256(h, _mm256_set1_epi8(0x30)),
                ),
                _mm256_or_si256(
                    _mm256_and_si256(_mm256_srli_epi16::<4>(l1), nibble),
                    _mm256_srli_epi16::<2>(_mm256_and_si256(h, _mm256_set1_epi8(0xc0u8 as i8))),
                ),
            ];
            for (k, q) in codes.into_iter().enumerate() {
                let scale = _mm256_shuffle_epi8(sc, lane_of(2 * k, 2 * k + 1));
                let pairs = _mm256_maddubs_epi16(q, load_i8(&xq[4 * n + k]));
                acc = _mm256_add_epi32(acc, _mm256_madd_epi16(pairs, scale));
            }
        }
        let offsets = _mm256_madd_epi16(scales, load_sums(sums));
        let (dot, offset) = hsum2(acc, offsets);
        sink.q6_k(d, dx, dot - 32 * offset);
    }
}

/// A shuffle that fills the low 128-bit lane with i16 lane `low` of its own
/// lane, and the high one with i16 lane `high` of its own.
#[target_feature(enable = "avx2")]
pub(crate) fn lane_of(low: usize, high: usize) -> __m256i {
    let pick = |i: usize| (2 * i as i16) | ((2 * i as i16 + 1) << 8);
    _mm256_inserti128_si256::<1>(
        _mm256_castsi128_si256(_mm_set1_epi16(pick(low))),
        _mm_set1_epi16(pick(high)),
    )
}

/// Two f16s as f32s. F16C gives the same values as `f16::to_f32`, except that it
/// quiets a signalling NaN, and a NaN row comes out as the one canonical NaN
/// either way.
#[target_feature(enable = "f16c")]
pub(crate) fn halves(bytes: &[u8; 4]) -> [f32; 2] {
    let v = _mm_cvtph_ps(_mm_cvtsi32_si128(i32::from_le_bytes(*bytes)));
    [_mm_cvtss_f32(v), _mm_cvtss_f32(_mm_movehdup_ps(v))]
}

/// One f16 as an f32, as [`halves`] converts two.
#[target_feature(enable = "f16c")]
pub(crate) fn half(h: u16) -> f32 {
    _mm_cvtss_f32(_mm_cvtph_ps(_mm_cvtsi32_si128(i32::from(h))))
}

#[target_feature(enable = "avx")]
pub(crate) fn load(bytes: &[u8; 32]) -> __m256i {
    // SAFETY: 32 readable bytes; loadu has no alignment rule. AVX was dispatched.
    unsafe { _mm256_loadu_si256(bytes.as_ptr().cast()) }
}

#[target_feature(enable = "avx")]
pub(crate) fn load_i8(bytes: &[i8; 32]) -> __m256i {
    // SAFETY: 32 readable bytes; loadu has no alignment rule. AVX was dispatched.
    unsafe { _mm256_loadu_si256(bytes.as_ptr().cast()) }
}

#[target_feature(enable = "avx")]
pub(crate) fn load_sums(sums: &[i16; 16]) -> __m256i {
    // SAFETY: 32 readable bytes; loadu has no alignment rule. AVX was dispatched.
    unsafe { _mm256_loadu_si256(sums.as_ptr().cast()) }
}

#[target_feature(enable = "avx")]
pub(crate) fn load128(bytes: &[u8; 16]) -> __m128i {
    // SAFETY: 16 readable bytes; loadu has no alignment rule. AVX was dispatched.
    unsafe { _mm_loadu_si128(bytes.as_ptr().cast()) }
}

/// The exact sums of the eight i32 lanes of `a` and of `b` (no partial sum can
/// overflow: see the bounds in the callers).
#[target_feature(enable = "avx2")]
pub(crate) fn hsum2(a: __m256i, b: __m256i) -> (i32, i32) {
    // a0+a1, a2+a3, b0+b1, b2+b3 | a4+a5, a6+a7, b4+b5, b6+b7
    let pairs = _mm256_hadd_epi32(a, b);
    let quads = _mm_add_epi32(
        _mm256_castsi256_si128(pairs),
        _mm256_extracti128_si256::<1>(pairs),
    );
    let sums = _mm_hadd_epi32(quads, quads);
    (_mm_cvtsi128_si32(sums), _mm_extract_epi32::<1>(sums))
}
