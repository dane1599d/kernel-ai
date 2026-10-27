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
/// 2 * 63 * 2,032: far from overflow. Two super-blocks share one horizontal sum
/// ([`hsum4`]); their sinks still run in super-block order.
#[target_feature(enable = "avx2,fma,f16c")]
pub fn q4_k_dot(row: &[u8], x: &Q8KVec, sink: &mut impl Sink) {
    let blocks = row.as_chunks::<{ q4_k::BLOCK_BYTES }>().0;
    let xs = x.qs.as_chunks::<SUPER_LEN>().0;
    let sums = x.sums.as_chunks::<16>().0;
    let n = blocks.len();
    let mut i = 0;
    while i + 1 < n {
        let (acc0, mins0, d0) = q4_k_block(&blocks[i], &xs[i], &sums[i]);
        let (acc1, mins1, d1) = q4_k_block(&blocks[i + 1], &xs[i + 1], &sums[i + 1]);
        let [a0, b0, a1, b1] = hsum4(acc0, mins0, acc1, mins1);
        sink.q4_k(d0[0], d0[1], x.scales[i], a0, b0);
        sink.q4_k(d1[0], d1[1], x.scales[i + 1], a1, b1);
        i += 2;
    }
    if i < n {
        let (acc, mins, d) = q4_k_block(&blocks[i], &xs[i], &sums[i]);
        let (a, b) = hsum2(acc, mins);
        sink.q4_k(d[0], d[1], x.scales[i], a, b);
    }
}

/// Four Q4_K rows of one length against `x`, each row's total as
/// [`crate::kquant::Total`] adds its super-blocks up: the integers are
/// [`q4_k_block`]'s, and lane r of each float op below is row r's scalar op, in
/// the same order (`(d * dx) * a - (dmin * dx) * b`, then the add), so the same
/// bits. The rows share x's loads and the scale shuffles, the horizontal sums,
/// the f16 converts and the float tail, which one row pays alone in
/// [`q4_k_dot`]. Four rows read at once are four streams the hardware
/// prefetcher follows badly (from DRAM about half as fast as one row), so the
/// next four rows are prefetched in order, a block's share per block.
#[target_feature(enable = "avx2,fma,f16c")]
pub fn q4_k_dot4(rows: [&[u8]; 4], x: &Q8KVec) -> [f32; 4] {
    let [r0, r1, r2, r3] = rows.map(|r| r.as_chunks::<{ q4_k::BLOCK_BYTES }>().0);
    let xs = x.qs.as_chunks::<SUPER_LEN>().0;
    let sums = x.sums.as_chunks::<16>().0;
    // Even i32 lanes to the low half, odd ones to the high half.
    let split = _mm256_setr_epi32(0, 2, 4, 6, 1, 3, 5, 7);
    let nibble = _mm256_set1_epi8(15);
    let mut total = _mm_setzero_ps();
    for (i, ((xq, sums), &dx)) in xs.iter().zip(sums).zip(&x.scales).enumerate() {
        let blocks = [&r0[i], &r1[i], &r2[i], &r3[i]];
        // Prefetch never faults, so the rows past the matrix's end are harmless.
        let next = rows[0]
            .as_ptr()
            .wrapping_add(4 * rows[0].len() + 4 * q4_k::BLOCK_BYTES * i);
        for line in 0..4 * q4_k::BLOCK_BYTES / 64 {
            _mm_prefetch::<_MM_HINT_T0>(next.wrapping_add(64 * line).cast());
        }
        let xq = xq.as_chunks::<32>().0;
        // Each row's scales and minimum term, as `q4_k_block` lays them out.
        let mut sc = [_mm256_setzero_si256(); 4];
        let mut mins = [_mm256_setzero_si256(); 4];
        for (r, block) in blocks.iter().enumerate() {
            let (s, m) = q4_k::packed_scales(block[4..q4_k::CODES].try_into().unwrap());
            sc[r] = _mm256_broadcastsi128_si256(_mm_cvtepu8_epi16(_mm_cvtsi64_si128(s as i64)));
            let m = _mm_cvtsi64_si128(m as i64);
            let m = _mm256_cvtepu8_epi16(_mm_unpacklo_epi8(m, m));
            mins[r] = _mm256_madd_epi16(m, load_sums(sums));
        }
        // Run g of every row against the same x bytes, as `q4_k_block` adds them.
        let mut acc = [_mm256_setzero_si256(); 4];
        for g in 0..4 {
            let (xlo, xhi) = (load_i8(&xq[2 * g]), load_i8(&xq[2 * g + 1]));
            let (plo, phi) = (lane_of(2 * g, 2 * g), lane_of(2 * g + 1, 2 * g + 1));
            for (r, block) in blocks.iter().enumerate() {
                let q = load(block[q4_k::CODES + 32 * g..][..32].try_into().unwrap());
                let lo = _mm256_and_si256(q, nibble);
                let hi = _mm256_and_si256(_mm256_srli_epi16::<4>(q), nibble);
                let lo = _mm256_madd_epi16(
                    _mm256_maddubs_epi16(lo, xlo),
                    _mm256_shuffle_epi8(sc[r], plo),
                );
                let hi = _mm256_madd_epi16(
                    _mm256_maddubs_epi16(hi, xhi),
                    _mm256_shuffle_epi8(sc[r], phi),
                );
                acc[r] = _mm256_add_epi32(acc[r], _mm256_add_epi32(lo, hi));
            }
        }
        let [a0, a1, a2, a3] = acc;
        let [b0, b1, b2, b3] = mins;
        // Per 128-bit lane: a0, b0, a1, b1 over lanes 0-3 | over lanes 4-7, and
        // the same for rows 2 and 3; then exact sums a0, b0, ..., a3, b3.
        let p01 = _mm256_hadd_epi32(_mm256_hadd_epi32(a0, b0), _mm256_hadd_epi32(a1, b1));
        let p23 = _mm256_hadd_epi32(_mm256_hadd_epi32(a2, b2), _mm256_hadd_epi32(a3, b3));
        let ints = _mm256_add_epi32(
            _mm256_permute2x128_si256::<0x20>(p01, p23),
            _mm256_permute2x128_si256::<0x31>(p01, p23),
        );
        let ints = _mm256_permutevar8x32_epi32(ints, split);
        let a = _mm_cvtepi32_ps(_mm256_castsi256_si128(ints));
        let b = _mm_cvtepi32_ps(_mm256_extracti128_si256::<1>(ints));
        // Each row's d and dmin, as `halves` converts them.
        let word =
            |block: &[u8; q4_k::BLOCK_BYTES]| i32::from_le_bytes(*block.first_chunk().unwrap());
        let scales = _mm256_cvtph_ps(_mm_setr_epi32(
            word(&r0[i]),
            word(&r1[i]),
            word(&r2[i]),
            word(&r3[i]),
        ));
        let scales = _mm256_permutevar8x32_ps(scales, split);
        let (d, dmin) = (
            _mm256_castps256_ps128(scales),
            _mm256_extractf128_ps::<1>(scales),
        );
        let dx = _mm_set1_ps(dx);
        let terms = _mm_sub_ps(
            _mm_mul_ps(_mm_mul_ps(d, dx), a),
            _mm_mul_ps(_mm_mul_ps(dmin, dx), b),
        );
        total = _mm_add_ps(total, terms);
    }
    let mut out = [0.0; 4];
    // SAFETY: 16 writable bytes; storeu has no alignment rule.
    unsafe { _mm_storeu_ps(out.as_mut_ptr(), total) };
    out
}

/// One Q4_K super-block's lanes against `xq`: the codes times x by sub-block
/// scale, the minimums times x's sums, and `[d, dmin]`.
#[target_feature(enable = "avx2,fma,f16c")]
#[inline]
fn q4_k_block(
    block: &[u8; q4_k::BLOCK_BYTES],
    xq: &[i8; SUPER_LEN],
    sums: &[i16; 16],
) -> (__m256i, __m256i, [f32; 2]) {
    let nibble = _mm256_set1_epi8(15);
    let d = halves(block[..4].try_into().unwrap());
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
    (acc, _mm256_madd_epi16(m, load_sums(sums)), d)
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

/// The exact sums of the eight i32 lanes of each of `a0`, `b0`, `a1` and `b1`,
/// as [`hsum2`] gives two of them (no partial sum can overflow: see the bounds
/// in the callers).
#[target_feature(enable = "avx2")]
pub(crate) fn hsum4(a0: __m256i, b0: __m256i, a1: __m256i, b1: __m256i) -> [i32; 4] {
    // Pairs of each, then quads: a0, b0, a1, b1 over lanes 0-3 | over lanes 4-7.
    let quads = _mm256_hadd_epi32(_mm256_hadd_epi32(a0, b0), _mm256_hadd_epi32(a1, b1));
    let sums = _mm_add_epi32(
        _mm256_castsi256_si128(quads),
        _mm256_extracti128_si256::<1>(quads),
    );
    [
        _mm_cvtsi128_si32(sums),
        _mm_extract_epi32::<1>(sums),
        _mm_extract_epi32::<2>(sums),
        _mm_extract_epi32::<3>(sums),
    ]
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
