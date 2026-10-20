//! Q6_K blocks, as GGUF stores them: super-blocks of 256 values in 210 bytes.
//!
//! A super-block is 128 bytes of low code nibbles (`ql`), 64 bytes of high code
//! bit pairs (`qh`), 16 signed 8-bit scales and an f16 scale `d`. Its values are
//! sixteen sub-blocks of 16; value i of sub-block j is `d * sc[j] * (q[i] - 32)`
//! with a 6-bit code q. Each half n of 128 values uses `ql[64n..64n + 64]` and
//! `qh[32n..32n + 32]`: for l < 32, quarter k (32 values) takes the low nibble of
//! `ql[l + 32(k % 2)]` for k < 2 and the high nibble for k >= 2, and bits 2k and
//! 2k+1 of `qh[l]` as its top two bits.

use crate::f16;
use crate::kquant::Sink;
use crate::q8k::{Q8KVec, SUPER_LEN};

pub const BLOCK_BYTES: usize = 210;
/// Offsets of `qh`, the scales and `d`.
pub(crate) const HIGH: usize = 128;
pub(crate) const SCALES: usize = 192;
pub(crate) const D: usize = 208;

/// Bytes a row of `len` values takes; `len` is a whole number of super-blocks.
pub fn row_bytes(len: usize) -> usize {
    assert!(len.is_multiple_of(SUPER_LEN), "a Q6_K row of {len} values");
    len / SUPER_LEN * BLOCK_BYTES
}

pub(crate) fn header(block: &[u8; BLOCK_BYTES]) -> (f32, [i8; 16]) {
    let d = f16::to_f32(u16::from_le_bytes([block[D], block[D + 1]]));
    let mut sc = [0i8; 16];
    for (s, &b) in sc.iter_mut().zip(&block[SCALES..D]) {
        *s = b as i8;
    }
    (d, sc)
}

/// The 6-bit code of value `i` (0-255) of a super-block.
pub(crate) fn code(block: &[u8; BLOCK_BYTES], i: usize) -> u8 {
    let (n, k, l) = (i / 128, (i % 128) / 32, i % 32);
    let low = block[64 * n + 32 * (k % 2) + l] >> (4 * (k / 2));
    let high = block[HIGH + 32 * n + l] >> (2 * k);
    (low & 15) | ((high & 3) << 4)
}

/// Expands Q6_K super-blocks into `out`, 256 values per super-block.
pub fn dequantize_row(blocks: &[u8], out: &mut [f32]) {
    assert_eq!(blocks.len(), row_bytes(out.len()));
    let outs = out.as_chunks_mut::<SUPER_LEN>().0;
    for (block, out) in blocks.as_chunks::<BLOCK_BYTES>().0.iter().zip(outs) {
        let (d, sc) = header(block);
        for (i, o) in out.iter_mut().enumerate() {
            let scale = d * f32::from(sc[i / 16]);
            *o = scale * f32::from(i16::from(code(block, i)) - 32);
        }
    }
}

/// Quantises `x` into Q6_K super-blocks. A plain scheme for tests and generated
/// models, not llama.cpp's search: each sub-block's step is max|x| / 31, and the
/// 8-bit scales are rounded against the largest step.
pub fn quantize_row(x: &[f32], out: &mut [u8]) {
    assert_eq!(out.len(), row_bytes(x.len()));
    let blocks = out.as_chunks_mut::<BLOCK_BYTES>().0;
    for (x, block) in x.as_chunks::<SUPER_LEN>().0.iter().zip(blocks) {
        let mut step = [0.0f32; 16];
        for (s, x) in step.iter_mut().zip(x.as_chunks::<16>().0) {
            *s = x.iter().fold(0.0f32, |m, &v| m.max(v.abs())) / 31.0;
        }
        let top = step.iter().fold(0.0f32, |m, &v| m.max(v));
        let d16 = f16::from_f32(top / 127.0);
        let d = f16::to_f32(d16);
        let mut sc = [0i8; 16];
        if top > 0.0 && top.is_finite() {
            for (q, &s) in sc.iter_mut().zip(&step) {
                *q = crate::q8_0::round_even(s * 127.0 / top).clamp(0.0, 127.0) as i8;
            }
        }
        block.fill(0);
        for (i, &v) in x.iter().enumerate() {
            let scale = d * f32::from(sc[i / 16]);
            let q = if scale > 0.0 {
                crate::q8_0::round_even(v / scale).clamp(-32.0, 31.0)
            } else {
                0.0
            };
            let q = (q as i8 + 32) as u8;
            let (n, k, l) = (i / 128, (i % 128) / 32, i % 32);
            block[64 * n + 32 * (k % 2) + l] |= (q & 15) << (4 * (k / 2));
            block[HIGH + 32 * n + l] |= (q >> 4) << (2 * k);
        }
        for (b, &s) in block[SCALES..D].iter_mut().zip(&sc) {
            *b = s as u8;
        }
        block[D..].copy_from_slice(&d16.to_le_bytes());
    }
}

/// One row against `x`, in plain Rust: the reference every tier must equal. Per
/// super-block, `s` = sum over sub-blocks of sc * ((codes - 32) . x), exact, goes
/// to `sink` in super-block order.
pub(crate) fn dot_baseline(row: &[u8], x: &Q8KVec, sink: &mut impl Sink) {
    let blocks = row.as_chunks::<BLOCK_BYTES>().0.iter();
    let xs = x.qs.as_chunks::<SUPER_LEN>().0;
    let sums = x.sums.as_chunks::<16>().0;
    for (((block, xq), sums), &dx) in blocks.zip(xs).zip(sums).zip(&x.scales) {
        let (d, sc) = header(block);
        let mut s = 0i32;
        for (j, xq) in xq.as_chunks::<16>().0.iter().enumerate() {
            let dot: i32 = (0..16)
                .map(|l| i32::from(code(block, 16 * j + l)) * i32::from(xq[l]))
                .sum();
            s += i32::from(sc[j]) * dot;
        }
        s -= 32 * offset_term(&sc, sums);
        sink.q6_k(d, dx, s);
    }
}

/// Sum over sub-blocks of sc * (the sum of x's 16 bytes there).
pub(crate) fn offset_term(sc: &[i8; 16], sums: &[i16; 16]) -> i32 {
    sc.iter()
        .zip(sums)
        .map(|(&sc, &sum)| i32::from(sc) * i32::from(sum))
        .sum()
}
