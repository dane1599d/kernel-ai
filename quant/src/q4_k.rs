//! Q4_K blocks, as GGUF stores them: super-blocks of 256 values in 144 bytes.
//!
//! A super-block is an f16 scale `d`, an f16 `dmin`, 12 bytes holding eight 6-bit
//! scales and eight 6-bit minimums, and 128 bytes of 4-bit codes. Its 256 values
//! are eight sub-blocks of 32; value i of sub-block j is
//! `d * sc[j] * q[i] - dmin * m[j]`. Codes come in four runs of 32 bytes: run g
//! holds sub-block 2g in its low nibbles and sub-block 2g+1 in its high ones.

use crate::f16;
use crate::kquant::Sink;
use crate::q8k::{Q8KVec, SUPER_LEN};

pub const BLOCK_BYTES: usize = 144;
/// Offset of the 128 code bytes.
pub(crate) const CODES: usize = 16;

/// Bytes a row of `len` values takes; `len` is a whole number of super-blocks.
pub fn row_bytes(len: usize) -> usize {
    assert!(len.is_multiple_of(SUPER_LEN), "a Q4_K row of {len} values");
    len / SUPER_LEN * BLOCK_BYTES
}

/// The eight scales and eight minimums of a super-block, each at most 63. The
/// first four of each are the low six bits of bytes 0-3 and 4-7; the last four
/// take their low four bits from a nibble of bytes 8-11 and their top two from the
/// top two bits of bytes 0-3 (scales) or 4-7 (minimums).
pub(crate) fn scales_and_mins(packed: &[u8; 12]) -> ([u8; 8], [u8; 8]) {
    let mut sc = [0u8; 8];
    let mut m = [0u8; 8];
    for j in 0..4 {
        sc[j] = packed[j] & 63;
        m[j] = packed[j + 4] & 63;
        sc[j + 4] = (packed[j + 8] & 15) | ((packed[j] >> 6) << 4);
        m[j + 4] = (packed[j + 8] >> 4) | ((packed[j + 4] >> 6) << 4);
    }
    (sc, m)
}

pub(crate) fn header(block: &[u8; BLOCK_BYTES]) -> (f32, f32, [u8; 8], [u8; 8]) {
    let d = f16::to_f32(u16::from_le_bytes([block[0], block[1]]));
    let dmin = f16::to_f32(u16::from_le_bytes([block[2], block[3]]));
    let packed: &[u8; 12] = block[4..CODES].try_into().expect("12 bytes");
    let (sc, m) = scales_and_mins(packed);
    (d, dmin, sc, m)
}

/// Expands Q4_K super-blocks into `out`, 256 values per super-block.
pub fn dequantize_row(blocks: &[u8], out: &mut [f32]) {
    assert_eq!(blocks.len(), row_bytes(out.len()));
    let outs = out.as_chunks_mut::<SUPER_LEN>().0;
    for (block, out) in blocks.as_chunks::<BLOCK_BYTES>().0.iter().zip(outs) {
        let (d, dmin, sc, m) = header(block);
        let runs = block[CODES..].as_chunks::<32>().0;
        for (g, (run, out)) in runs.iter().zip(out.as_chunks_mut::<64>().0).enumerate() {
            let (lo, hi) = out.split_at_mut(32);
            for (j, out, shift) in [(2 * g, lo, 0), (2 * g + 1, hi, 4)] {
                let (scale, min) = (d * f32::from(sc[j]), dmin * f32::from(m[j]));
                for (o, &q) in out.iter_mut().zip(run) {
                    *o = scale * f32::from((q >> shift) & 15) - min;
                }
            }
        }
    }
}

/// Quantises `x` into Q4_K super-blocks. A plain scheme for tests and generated
/// models, not llama.cpp's search: each sub-block spans its minimum (at most 0)
/// to its maximum in 15 steps, and the 6-bit scales and minimums are rounded
/// against the largest of each.
pub fn quantize_row(x: &[f32], out: &mut [u8]) {
    assert_eq!(out.len(), row_bytes(x.len()));
    let blocks = out.as_chunks_mut::<BLOCK_BYTES>().0;
    for (x, block) in x.as_chunks::<SUPER_LEN>().0.iter().zip(blocks) {
        let mut step = [0.0f32; 8];
        let mut floor = [0.0f32; 8];
        for (j, x) in x.as_chunks::<32>().0.iter().enumerate() {
            let lo = x.iter().fold(0.0f32, |m, &v| m.min(v));
            let hi = x.iter().fold(0.0f32, |m, &v| m.max(v));
            step[j] = (hi - lo) / 15.0;
            floor[j] = -lo;
        }
        let (d, sc) = six_bit(&step);
        let (dmin, m) = six_bit(&floor);
        let (d16, dmin16) = (f16::from_f32(d), f16::from_f32(dmin));
        block[..2].copy_from_slice(&d16.to_le_bytes());
        block[2..4].copy_from_slice(&dmin16.to_le_bytes());
        let mut packed = [0u8; 12];
        for j in 0..4 {
            packed[j] = sc[j] | ((sc[j + 4] >> 4) << 6);
            packed[j + 4] = m[j] | ((m[j + 4] >> 4) << 6);
            packed[j + 8] = (sc[j + 4] & 15) | ((m[j + 4] & 15) << 4);
        }
        block[4..CODES].copy_from_slice(&packed);
        let (d, dmin) = (f16::to_f32(d16), f16::to_f32(dmin16));
        let codes = &mut block[CODES..];
        codes.fill(0);
        for (i, &v) in x.iter().enumerate() {
            let j = i / 32;
            let scale = d * f32::from(sc[j]);
            let q = if scale > 0.0 {
                crate::q8_0::round_even((v + dmin * f32::from(m[j])) / scale).clamp(0.0, 15.0)
            } else {
                0.0
            } as u8;
            codes[(i / 64) * 32 + i % 32] |= q << (4 * ((i / 32) % 2));
        }
    }
}

/// A super-scale and eight 6-bit multiples of it that approximate `values`
/// (each at least 0).
fn six_bit(values: &[f32; 8]) -> (f32, [u8; 8]) {
    let top = values.iter().fold(0.0f32, |m, &v| m.max(v));
    let mut q = [0u8; 8];
    if top > 0.0 && top.is_finite() {
        for (q, &v) in q.iter_mut().zip(values) {
            *q = crate::q8_0::round_even(v * 63.0 / top).clamp(0.0, 63.0) as u8;
        }
    }
    (top / 63.0, q)
}

/// The eight scales and the eight minimums as the bytes of two u64s, from three
/// u32 words: the same values as [`scales_and_mins`], in a dozen operations.
#[cfg(all(target_arch = "x86_64", target_feature = "sse2"))]
pub(crate) fn packed_scales(packed: &[u8; 12]) -> (u64, u64) {
    let word =
        |i: usize| u32::from_le_bytes([packed[i], packed[i + 1], packed[i + 2], packed[i + 3]]);
    let (w0, w1, w2) = (word(0), word(4), word(8));
    let low6 = 0x3f3f_3f3f;
    let (nib, top) = (0x0f0f_0f0f, 0x0303_0303);
    let sc_hi = (w2 & nib) | (((w0 >> 6) & top) << 4);
    let m_hi = ((w2 >> 4) & nib) | (((w1 >> 6) & top) << 4);
    let sc = u64::from(w0 & low6) | (u64::from(sc_hi) << 32);
    let m = u64::from(w1 & low6) | (u64::from(m_hi) << 32);
    (sc, m)
}

/// One row against `x`, in plain Rust: the reference every tier must equal.
/// Per super-block, `a` = sum over sub-blocks of sc * (codes . x) and `b` = sum of
/// m * (sum of x's bytes), both exact, go to `sink` in super-block order.
pub(crate) fn dot_baseline(row: &[u8], x: &Q8KVec, sink: &mut impl Sink) {
    let blocks = row.as_chunks::<BLOCK_BYTES>().0.iter();
    let xs = x.qs.as_chunks::<SUPER_LEN>().0;
    let sums = x.sums.as_chunks::<16>().0;
    for (((block, xq), sums), &dx) in blocks.zip(xs).zip(sums).zip(&x.scales) {
        let (d, dmin, sc, m) = header(block);
        let runs = block[CODES..].as_chunks::<32>().0;
        let mut a = 0i32;
        for (g, (run, xq)) in runs.iter().zip(xq.as_chunks::<64>().0).enumerate() {
            let (mut lo, mut hi) = (0i32, 0i32);
            for (l, &q) in run.iter().enumerate() {
                lo += i32::from(q & 15) * i32::from(xq[l]);
                hi += i32::from(q >> 4) * i32::from(xq[l + 32]);
            }
            a += i32::from(sc[2 * g]) * lo + i32::from(sc[2 * g + 1]) * hi;
        }
        sink.q4_k(d, dmin, dx, a, min_term(&m, sums));
    }
}

/// Sum over sub-blocks of m * (the sum of x's 32 bytes there), from the sums of 16.
pub(crate) fn min_term(m: &[u8; 8], sums: &[i16; 16]) -> i32 {
    let pairs = sums.as_chunks::<2>().0;
    m.iter()
        .zip(pairs)
        .map(|(&m, s)| i32::from(m) * (i32::from(s[0]) + i32::from(s[1])))
        .sum()
}
