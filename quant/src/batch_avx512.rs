//! The K-quant products of one row with a group of tokens in AVX-512, 64 bytes of
//! a token's activations at a time. Codes are unpacked once per block, and the
//! i16 pairs VPMADDUBSW makes are multiplied by their scales into i32 lanes by
//! VPMADDWD and an add (`avx512` tier) or one VPDPWSSD (`avx512+vnni`). Q4_K puts
//! two sub-blocks in one register (a run's low nibbles below, high nibbles
//! above); Q6_K follows [`crate::kquant_avx512`]. Every lane adds at most as many
//! products as an AVX2 kernel's lane, so the same bounds hold, and the lanes sum
//! to the integers the baseline computes.
//!
//! Measured on the 7700X with every block in cache, 32 tokens of a 2560-wide
//! matrix, one thread: Q4_K about 80-89 and Q6_K about 78-80 GMAC/s, against
//! 62-77 and 69-73 for the AVX2 kernels. A 512-bit Q8_0 kernel (two tokens to a
//! register) measured slower than the AVX2 one, which these tiers run for Q8_0,
//! as they do for one token.

use crate::batch_avx2::{Tokens, hsums};
use crate::kquant::Sink;
use crate::kquant_avx2::{half, halves, load_sums, load128};
use crate::kquant_avx512::{
    HIGH_SHIFTS, SCALE_LANES, dpwssd, fold, high_bits, load256, load512, load512_i8, load512_u16,
    madd_add,
};
use crate::q8k::Q8KVec;
use crate::{q4_k, q6_k};
use core::arch::x86_64::*;

/// The scale lane each i16 lane of run g's product takes: sub-block 2g for the
/// low 256 bits (low nibbles), 2g + 1 for the high 256 (high nibbles).
const PAIR_LANES: [[u16; 32]; 4] = {
    let mut lanes = [[0u16; 32]; 4];
    let mut g = 0;
    while g < 4 {
        let mut i = 0;
        while i < 32 {
            lanes[g][i] = (2 * g + i / 16) as u16;
            i += 1;
        }
        g += 1;
    }
    lanes
};

/// Shift counts that bring the high nibbles down in the high 256 bits.
const NIBBLE_SHIFTS: [u16; 32] = {
    let mut shifts = [0u16; 32];
    let mut i = 16;
    while i < 32 {
        shifts[i] = 4;
        i += 1;
    }
    shifts
};

macro_rules! k_kernels {
    ($q4:ident, $q6:ident, $features:literal, $acc:ident) => {
        /// One Q4_K row against each of `xs`. A lane adds 4 runs' pairs of
        /// products of at most 2 * 15 * 127 * 63.
        #[target_feature(enable = $features)]
        pub fn $q4<const N: usize, S: Sink>(row: &[u8], xs: &[Q8KVec; N], sinks: &mut [S; N]) {
            let nibble = _mm512_set1_epi8(15);
            let shifts = load512_u16(&NIBBLE_SHIFTS);
            let lanes = PAIR_LANES.map(|l| load512_u16(&l));
            let blocks = row.as_chunks::<{ q4_k::BLOCK_BYTES }>().0;
            let tokens = Tokens::new(xs, blocks.len());
            for (b, block) in blocks.iter().enumerate() {
                let [d, dmin] = halves(block[..4].try_into().unwrap());
                let (sc, m) = q4_k::packed_scales(block[4..q4_k::CODES].try_into().unwrap());
                let sc = _mm512_castsi128_si512(_mm_cvtepu8_epi16(_mm_cvtsi64_si128(sc as i64)));
                let m = _mm_cvtsi64_si128(m as i64);
                let m = _mm256_cvtepu8_epi16(_mm_unpacklo_epi8(m, m));
                let runs = block[q4_k::CODES..].as_chunks::<32>().0;
                let xb = tokens.bytes::<64, 4>(b);
                let mut acc = [_mm512_setzero_si512(); N];
                for (g, run) in runs.iter().enumerate() {
                    let q = _mm512_broadcast_i64x4(load256(run));
                    let q = _mm512_and_si512(_mm512_srlv_epi16(q, shifts), nibble);
                    let scale = _mm512_permutexvar_epi16(lanes[g], sc);
                    for (acc, xb) in acc.iter_mut().zip(&xb) {
                        let pairs = _mm512_maddubs_epi16(q, load512_i8(&xb[g]));
                        *acc = $acc(*acc, pairs, scale);
                    }
                }
                let sums = tokens.sums(b);
                let mins: [__m256i; N] =
                    core::array::from_fn(|t| _mm256_madd_epi16(m, load_sums(sums[t])));
                let (a, bias) = (hsums(&acc.map(|v| fold(v))), hsums(&mins));
                let dx = tokens.scales(b);
                S::q4_k_each(sinks, d, dmin, &dx, &a, &bias);
            }
        }

        /// One Q6_K row against each of `xs`, as
        /// [`crate::kquant_avx512::q6_k_dot`] runs each.
        #[target_feature(enable = $features)]
        pub fn $q6<const N: usize, S: Sink>(row: &[u8], xs: &[Q8KVec; N], sinks: &mut [S; N]) {
            let nibble = _mm512_set1_epi8(15);
            let pair = _mm512_set1_epi8(3);
            let shifts = [load512_u16(&HIGH_SHIFTS[0]), load512_u16(&HIGH_SHIFTS[1])];
            let lanes = SCALE_LANES.map(|l| load512_u16(&l));
            let blocks = row.as_chunks::<{ q6_k::BLOCK_BYTES }>().0;
            let tokens = Tokens::new(xs, blocks.len());
            for (b, block) in blocks.iter().enumerate() {
                let d = half(u16::from_le_bytes([block[q6_k::D], block[q6_k::D + 1]]));
                let scales = load128(block[q6_k::SCALES..q6_k::D].try_into().unwrap());
                let scales = _mm256_cvtepi8_epi16(scales);
                let wide = _mm512_castsi256_si512(scales);
                let low = block[..q6_k::HIGH].as_chunks::<64>().0;
                let high = block[q6_k::HIGH..q6_k::SCALES].as_chunks::<32>().0;
                let xb = tokens.bytes::<64, 4>(b);
                let mut acc = [_mm512_setzero_si512(); N];
                for n in 0..2 {
                    let l = load512(&low[n]);
                    let h = _mm512_broadcast_i64x4(load256(&high[n]));
                    let q01 =
                        _mm512_or_si512(_mm512_and_si512(l, nibble), high_bits(h, shifts[0], pair));
                    let q23 = _mm512_or_si512(
                        _mm512_and_si512(_mm512_srli_epi16::<4>(l), nibble),
                        high_bits(h, shifts[1], pair),
                    );
                    for (k, q) in [q01, q23].into_iter().enumerate() {
                        let scale = _mm512_permutexvar_epi16(lanes[2 * n + k], wide);
                        for (acc, xb) in acc.iter_mut().zip(&xb) {
                            let pairs = _mm512_maddubs_epi16(q, load512_i8(&xb[2 * n + k]));
                            *acc = $acc(*acc, pairs, scale);
                        }
                    }
                }
                let sums = tokens.sums(b);
                let offsets: [__m256i; N] =
                    core::array::from_fn(|t| _mm256_madd_epi16(scales, load_sums(sums[t])));
                let (dot, offset) = (hsums(&acc.map(|v| fold(v))), hsums(&offsets));
                let dx = tokens.scales(b);
                let s = core::array::from_fn(|t| dot[t] - 32 * offset[t]);
                S::q6_k_each(sinks, d, &dx, &s);
            }
        }
    };
}

// As in kquant_avx512: the avx512 tier's kernels must not enable avx512vnni,
// which AVX-512 CPUs without it (Skylake-SP) would fault on.
k_kernels!(
    q4_k_dot_n,
    q6_k_dot_n,
    "avx2,fma,f16c,avx512f,avx512bw,avx512vl",
    madd_add
);
k_kernels!(
    q4_k_dot_n_vnni,
    q6_k_dot_n_vnni,
    "avx2,fma,f16c,avx512f,avx512bw,avx512vl,avx512vnni",
    dpwssd
);
