//! The Q6_K row product in AVX-512, 64 codes at a time: VPMADDUBSW multiplies
//! unsigned codes by x's signed bytes in i16 pairs (at most 2 * 63 * 127), and
//! the pairs are multiplied by their sub-block's scale into i32 lanes, by
//! VPMADDWD and an add in the `avx512` tier or by one VPDPWSSD in the
//! `avx512+vnni` tier. Each lane holds as many products as an AVX2 kernel's lane,
//! so the same bounds hold, and a super-block's lanes sum to the same exact
//! integers the baseline computes.
//!
//! Q4_K has no AVX-512 kernel: on the 7700X its 512-bit and 256-bit VNNI kernels
//! measured about 8% slower than the AVX2 one, which every tier then runs.

use crate::kquant::Sink;
use crate::kquant_avx2::{half, hsum2, load_sums, load128};
use crate::q6_k;
use crate::q8k::{Q8KVec, SUPER_LEN};
use core::arch::x86_64::*;

/// Which of the 16 scales each i16 lane of a 64-code product takes: half n,
/// quarters 0-1 then 2-3, 8 lanes (16 codes) per scale.
const SCALE_LANES: [[u16; 32]; 4] = {
    let mut lanes = [[0u16; 32]; 4];
    let mut v = 0;
    while v < 4 {
        let mut i = 0;
        while i < 32 {
            lanes[v][i] = (4 * v + i / 8) as u16;
            i += 1;
        }
        v += 1;
    }
    lanes
};

/// Shift counts that bring quarter k's bit pair of `qh` down to bits 0-1: the low
/// 256 bits hold quarter 0 (or 2), the high 256 bits quarter 1 (or 3).
const HIGH_SHIFTS: [[u16; 32]; 2] = {
    let mut shifts = [[0u16; 32]; 2];
    let mut i = 0;
    while i < 32 {
        shifts[0][i] = if i < 16 { 0 } else { 2 };
        shifts[1][i] = if i < 16 { 4 } else { 6 };
        i += 1;
    }
    shifts
};

macro_rules! q6_k_kernel {
    ($name:ident, $features:literal, $acc:ident) => {
        /// One Q6_K row against `x`. Per half, `ql`'s 64 bytes and `qh` (in both
        /// 256-bit halves) give quarters 0-1 from the low nibbles and 2-3 from the
        /// high ones, each against x's 64 bytes there.
        #[target_feature(enable = $features)]
        pub fn $name(row: &[u8], x: &Q8KVec, sink: &mut impl Sink) {
            let nibble = _mm512_set1_epi8(15);
            let pair = _mm512_set1_epi8(3);
            let shifts = [load512_u16(&HIGH_SHIFTS[0]), load512_u16(&HIGH_SHIFTS[1])];
            let lanes = SCALE_LANES.map(|l| load512_u16(&l));
            let blocks = row.as_chunks::<{ q6_k::BLOCK_BYTES }>().0.iter();
            let xs = x.qs.as_chunks::<SUPER_LEN>().0;
            let sums = x.sums.as_chunks::<16>().0;
            for (((block, xq), sums), &dx) in blocks.zip(xs).zip(sums).zip(&x.scales) {
                let d = half(u16::from_le_bytes([block[q6_k::D], block[q6_k::D + 1]]));
                let scales = load128(block[q6_k::SCALES..q6_k::D].try_into().unwrap());
                let scales = _mm256_cvtepi8_epi16(scales);
                let offsets = _mm256_madd_epi16(scales, load_sums(sums));
                let scales = _mm512_castsi256_si512(scales);
                let low = block[..q6_k::HIGH].as_chunks::<64>().0;
                let high = block[q6_k::HIGH..q6_k::SCALES].as_chunks::<32>().0;
                let xq = xq.as_chunks::<64>().0;
                let mut acc = _mm512_setzero_si512();
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
                        let scale = _mm512_permutexvar_epi16(lanes[2 * n + k], scales);
                        let pairs = _mm512_maddubs_epi16(q, load512_i8(&xq[2 * n + k]));
                        acc = $acc(acc, pairs, scale);
                    }
                }
                let (dot, offset) = hsum2(fold(acc), offsets);
                sink.q6_k(d, dx, dot - 32 * offset);
            }
        }
    };
}

// The avx512 tier's kernel must not enable avx512vnni: it runs on AVX-512 CPUs
// without VNNI (Skylake-SP), where a VPDPWSSD LLVM chose would fault, and no test
// on a CPU that has VNNI can catch that.
q6_k_kernel!(
    q6_k_dot,
    "avx2,fma,f16c,avx512f,avx512bw,avx512vl",
    madd_add
);
q6_k_kernel!(
    q6_k_dot_vnni,
    "avx2,fma,f16c,avx512f,avx512bw,avx512vl,avx512vnni",
    dpwssd
);

/// The 16 i32 lanes added in pairs into 8, exactly.
#[target_feature(enable = "avx512f")]
fn fold(v: __m512i) -> __m256i {
    _mm256_add_epi32(_mm512_castsi512_si256(v), _mm512_extracti64x4_epi64::<1>(v))
}

/// Bits 4-5 of each code: the bit pair of `qh` each lane's shift brings down.
/// The shifts move 16-bit lanes, but the mask keeps every bit in its own byte.
#[target_feature(enable = "avx512f,avx512bw")]
fn high_bits(qh: __m512i, shifts: __m512i, pair: __m512i) -> __m512i {
    let bits = _mm512_and_si512(_mm512_srlv_epi16(qh, shifts), pair);
    _mm512_slli_epi16::<4>(bits)
}

/// `acc` plus the i16 `pairs` times `scale`, summed in pairs into i32 lanes.
#[target_feature(enable = "avx512f,avx512bw")]
fn madd_add(acc: __m512i, pairs: __m512i, scale: __m512i) -> __m512i {
    _mm512_add_epi32(acc, _mm512_madd_epi16(pairs, scale))
}

/// [`madd_add`] in one VNNI instruction.
#[target_feature(enable = "avx512f,avx512vnni")]
fn dpwssd(acc: __m512i, pairs: __m512i, scale: __m512i) -> __m512i {
    _mm512_dpwssd_epi32(acc, pairs, scale)
}

#[target_feature(enable = "avx")]
fn load256(bytes: &[u8; 32]) -> __m256i {
    // SAFETY: 32 readable bytes; loadu has no alignment rule. The AVX-512 tier
    // this runs in was dispatched only with AVX2.
    unsafe { _mm256_loadu_si256(bytes.as_ptr().cast()) }
}

#[target_feature(enable = "avx512f")]
fn load512(bytes: &[u8; 64]) -> __m512i {
    // SAFETY: 64 readable bytes; loadu has no alignment rule. The AVX-512 tier
    // this runs in was dispatched only with AVX-512 F.
    unsafe { _mm512_loadu_si512(bytes.as_ptr().cast()) }
}

#[target_feature(enable = "avx512f")]
fn load512_i8(bytes: &[i8; 64]) -> __m512i {
    // SAFETY: 64 readable bytes; loadu has no alignment rule. The AVX-512 tier
    // this runs in was dispatched only with AVX-512 F.
    unsafe { _mm512_loadu_si512(bytes.as_ptr().cast()) }
}

#[target_feature(enable = "avx512f")]
fn load512_u16(words: &[u16; 32]) -> __m512i {
    // SAFETY: 64 readable bytes; loadu has no alignment rule. The AVX-512 tier
    // this runs in was dispatched only with AVX-512 F.
    unsafe { _mm512_loadu_si512(words.as_ptr().cast()) }
}
