//! The K-quant products of one row with a group of tokens in AVX2, and what the
//! AVX-512 kernels share. Each block's scales and codes are unpacked once and
//! multiplied by every token's bytes with the instructions of the one-token
//! kernels in [`crate::kquant_avx2`]: VPMADDUBSW multiplies unsigned codes by
//! x's signed bytes in i16 pairs, and VPMADDWD multiplies each pair by its
//! sub-block's scale into i32 lanes. Each token's lanes take the products the
//! one-token kernel's lanes take, so the same bounds hold, and they sum to the
//! integers the baseline computes; eight tokens' lanes are summed by one tree of
//! adds, and four tokens' float tails run in one SSE register
//! ([`Sink::q4_k_each`]).
//!
//! Measured on the 7700X with every block in cache, 32 tokens of a 2560-wide
//! matrix: Q4_K about 75 and Q6_K about 70 GMAC/s on one thread, against 44 and
//! 35 for the one-token kernel run per token. Summing products further in i16
//! (horizontal adds) before the scale multiply, which halves the multiplies,
//! measured slower here.

use crate::kquant::Sink;
use crate::kquant_avx2::{half, halves, load, load_sums, load128};
use crate::q8k::{Q8KVec, SUPER_LEN};
use crate::{q4_k, q6_k};
use core::arch::x86_64::*;

/// A group's activations, each token's cut to the row's super-blocks once, so
/// the block loop indexes them without further bounds checks.
pub(crate) struct Tokens<'a, const N: usize> {
    qs: [&'a [[i8; SUPER_LEN]]; N],
    sums: [&'a [[i16; 16]]; N],
    scales: [&'a [f32]; N],
}

impl<'a, const N: usize> Tokens<'a, N> {
    /// `xs` for a row of `blocks` super-blocks; each must have at least that
    /// many (the products check that they have exactly that many).
    #[inline(always)]
    pub(crate) fn new(xs: &'a [Q8KVec; N], blocks: usize) -> Self {
        Self {
            qs: core::array::from_fn(|t| &xs[t].qs.as_chunks().0[..blocks]),
            sums: core::array::from_fn(|t| &xs[t].sums.as_chunks().0[..blocks]),
            scales: core::array::from_fn(|t| &xs[t].scales[..blocks]),
        }
    }

    /// Each token's bytes for super-block `b`, in runs of `R` bytes.
    #[inline(always)]
    pub(crate) fn bytes<const R: usize, const RUNS: usize>(
        &self,
        b: usize,
    ) -> [&'a [[i8; R]; RUNS]; N] {
        core::array::from_fn(|t| {
            self.qs[t][b]
                .as_chunks::<R>()
                .0
                .try_into()
                .expect("whole runs")
        })
    }

    /// Each token's sixteen byte sums for super-block `b`.
    #[inline(always)]
    pub(crate) fn sums(&self, b: usize) -> [&'a [i16; 16]; N] {
        core::array::from_fn(|t| &self.sums[t][b])
    }

    /// Each token's scale for super-block `b`.
    #[inline(always)]
    pub(crate) fn scales(&self, b: usize) -> [f32; N] {
        core::array::from_fn(|t| self.scales[t][b])
    }
}

/// The exact sum of each vector's eight i32 lanes (no partial sum overflows:
/// see the bounds in the callers): eight vectors to one tree of horizontal adds
/// and one store, then four, then one at a time.
#[target_feature(enable = "avx2")]
pub(crate) fn hsums<const N: usize>(v: &[__m256i; N]) -> [i32; N] {
    let mut out = [0i32; N];
    let mut done = 0;
    while done + 8 <= N {
        let q = &v[done..done + 8];
        // Lanes 0-3 of four vectors summed in the low half, 4-7 in the high.
        let low = _mm256_hadd_epi32(_mm256_hadd_epi32(q[0], q[1]), _mm256_hadd_epi32(q[2], q[3]));
        let high = _mm256_hadd_epi32(_mm256_hadd_epi32(q[4], q[5]), _mm256_hadd_epi32(q[6], q[7]));
        let s = _mm256_add_epi32(
            _mm256_permute2x128_si256::<0x20>(low, high),
            _mm256_permute2x128_si256::<0x31>(low, high),
        );
        let o: &mut [i32; 8] = (&mut out[done..done + 8]).try_into().unwrap();
        // SAFETY: 32 writable bytes; storeu has no alignment rule. AVX2 was
        // dispatched.
        unsafe { _mm256_storeu_si256(o.as_mut_ptr().cast(), s) };
        done += 8;
    }
    while done + 4 <= N {
        let q = &v[done..done + 4];
        let h = _mm256_hadd_epi32(_mm256_hadd_epi32(q[0], q[1]), _mm256_hadd_epi32(q[2], q[3]));
        let s = _mm_add_epi32(_mm256_castsi256_si128(h), _mm256_extracti128_si256::<1>(h));
        let o: &mut [i32; 4] = (&mut out[done..done + 4]).try_into().unwrap();
        // SAFETY: 16 writable bytes; storeu has no alignment rule. AVX2 was
        // dispatched.
        unsafe { _mm_storeu_si128(o.as_mut_ptr().cast(), s) };
        done += 4;
    }
    for (v, o) in v[done..].iter().zip(&mut out[done..]) {
        let s = _mm_add_epi32(
            _mm256_castsi256_si128(*v),
            _mm256_extracti128_si256::<1>(*v),
        );
        let s = _mm_hadd_epi32(s, s);
        *o = _mm_cvtsi128_si32(_mm_hadd_epi32(s, s));
    }
    out
}

#[target_feature(enable = "avx")]
fn load_mask(bytes: &[u8; 32]) -> __m256i {
    // SAFETY: 32 readable bytes; loadu has no alignment rule. AVX was dispatched.
    unsafe { _mm256_loadu_si256(bytes.as_ptr().cast()) }
}

#[target_feature(enable = "avx")]
pub(crate) fn load_x(bytes: &[i8; 32]) -> __m256i {
    // SAFETY: 32 readable bytes; loadu has no alignment rule. AVX was dispatched.
    unsafe { _mm256_loadu_si256(bytes.as_ptr().cast()) }
}

/// Q4_K's sub-block j's scale in every i16 lane, and Q6_K's quarter k's two
/// scales, one to each 128-bit lane, from the scales in both lanes.
const fn pick(low: usize, high: usize) -> [u8; 32] {
    let mut mask = [0u8; 32];
    let mut i = 0;
    while i < 16 {
        mask[i] = (2 * low + i % 2) as u8;
        mask[16 + i] = (2 * high + i % 2) as u8;
        i += 1;
    }
    mask
}
const SAME: [[u8; 32]; 8] = {
    let mut masks = [[0u8; 32]; 8];
    let mut j = 0;
    while j < 8 {
        masks[j] = pick(j, j);
        j += 1;
    }
    masks
};
const PAIRS: [[u8; 32]; 4] = [pick(0, 1), pick(2, 3), pick(4, 5), pick(6, 7)];

macro_rules! k_kernels {
    ($q4:ident, $q6:ident, $features:literal, $acc:ident) => {
        /// One Q4_K row against each of `xs`, giving each token the integers
        /// [`crate::kquant_avx2::q4_k_dot`] gives, with its instructions: a lane
        /// of a token's accumulator adds 8 products of at most
        /// 2 * 15 * 127 * 63.
        #[target_feature(enable = $features)]
        pub fn $q4<const N: usize, S: Sink>(row: &[u8], xs: &[Q8KVec; N], sinks: &mut [S; N]) {
            let nibble = _mm256_set1_epi8(15);
            let same = SAME.map(|m| load_mask(&m));
            let blocks = row.as_chunks::<{ q4_k::BLOCK_BYTES }>().0;
            let tokens = Tokens::new(xs, blocks.len());
            for (b, block) in blocks.iter().enumerate() {
                let [d, dmin] = halves(block[..4].try_into().unwrap());
                let (sc, m) = q4_k::packed_scales(block[4..q4_k::CODES].try_into().unwrap());
                let sc =
                    _mm256_broadcastsi128_si256(_mm_cvtepu8_epi16(_mm_cvtsi64_si128(sc as i64)));
                let m = _mm_cvtsi64_si128(m as i64);
                let m = _mm256_cvtepu8_epi16(_mm_unpacklo_epi8(m, m));
                let runs = block[q4_k::CODES..].as_chunks::<32>().0;
                let xb = tokens.bytes::<32, 8>(b);
                let mut acc = [_mm256_setzero_si256(); N];
                for (g, run) in runs.iter().enumerate() {
                    let q = load(run);
                    let lo = _mm256_and_si256(q, nibble);
                    let hi = _mm256_and_si256(_mm256_srli_epi16::<4>(q), nibble);
                    let sc_lo = _mm256_shuffle_epi8(sc, same[2 * g]);
                    let sc_hi = _mm256_shuffle_epi8(sc, same[2 * g + 1]);
                    for (acc, x) in acc.iter_mut().zip(&xb) {
                        *acc = $acc(*acc, _mm256_maddubs_epi16(lo, load_x(&x[2 * g])), sc_lo);
                        *acc = $acc(*acc, _mm256_maddubs_epi16(hi, load_x(&x[2 * g + 1])), sc_hi);
                    }
                }
                let sums = tokens.sums(b);
                let mins: [__m256i; N] =
                    core::array::from_fn(|t| _mm256_madd_epi16(m, load_sums(sums[t])));
                let (a, bias) = (hsums(&acc), hsums(&mins));
                S::q4_k_each(sinks, d, dmin, &tokens.scales(b), &a, &bias);
            }
        }

        /// One Q6_K row against each of `xs`, giving each token the integers
        /// [`crate::kquant_avx2::q6_k_dot`] gives, with its instructions: a lane
        /// of a token's accumulator adds 8 products of at most 2 * 16,002 * 128.
        #[target_feature(enable = $features)]
        pub fn $q6<const N: usize, S: Sink>(row: &[u8], xs: &[Q8KVec; N], sinks: &mut [S; N]) {
            let pairs = PAIRS.map(|m| load_mask(&m));
            let blocks = row.as_chunks::<{ q6_k::BLOCK_BYTES }>().0;
            let tokens = Tokens::new(xs, blocks.len());
            for (b, block) in blocks.iter().enumerate() {
                let d = half(u16::from_le_bytes([block[q6_k::D], block[q6_k::D + 1]]));
                let scales = load128(block[q6_k::SCALES..q6_k::D].try_into().unwrap());
                let scales = _mm256_cvtepi8_epi16(scales);
                let low = block[..q6_k::HIGH].as_chunks::<32>().0;
                let high = block[q6_k::HIGH..q6_k::SCALES].as_chunks::<32>().0;
                let xb = tokens.bytes::<32, 8>(b);
                let mut acc = [_mm256_setzero_si256(); N];
                for n in 0..2 {
                    let (l0, l1, h) = (load(&low[2 * n]), load(&low[2 * n + 1]), load(&high[n]));
                    let sc = if n == 0 {
                        _mm256_permute2x128_si256::<0x00>(scales, scales)
                    } else {
                        _mm256_permute2x128_si256::<0x11>(scales, scales)
                    };
                    for (k, q) in q6_codes(l0, l1, h).into_iter().enumerate() {
                        let scale = _mm256_shuffle_epi8(sc, pairs[k]);
                        for (acc, x) in acc.iter_mut().zip(&xb) {
                            let pairs = _mm256_maddubs_epi16(q, load_x(&x[4 * n + k]));
                            *acc = $acc(*acc, pairs, scale);
                        }
                    }
                }
                let sums = tokens.sums(b);
                let offsets: [__m256i; N] =
                    core::array::from_fn(|t| _mm256_madd_epi16(scales, load_sums(sums[t])));
                let (dot, offset) = (hsums(&acc), hsums(&offsets));
                let s = core::array::from_fn(|t| dot[t] - 32 * offset[t]);
                S::q6_k_each(sinks, d, &tokens.scales(b), &s);
            }
        }
    };
}

k_kernels!(q4_k_dot_n, q6_k_dot_n, "avx2,fma,f16c", madd_add);

/// `acc` plus the i16 `pairs` times `scale`, summed in pairs into i32 lanes.
#[target_feature(enable = "avx2")]
fn madd_add(acc: __m256i, pairs: __m256i, scale: __m256i) -> __m256i {
    _mm256_add_epi32(acc, _mm256_madd_epi16(pairs, scale))
}

/// A half's four quarters of 6-bit codes from its two runs of low nibbles and
/// its run of high bit pairs, as the one-token kernel puts them together: each
/// shift moves 16-bit lanes, but the masks keep every bit in its own byte.
#[target_feature(enable = "avx2")]
fn q6_codes(l0: __m256i, l1: __m256i, h: __m256i) -> [__m256i; 4] {
    let nibble = _mm256_set1_epi8(15);
    [
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
    ]
}
