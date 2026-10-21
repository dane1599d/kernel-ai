//! Test support for the K-quants: random and odd blocks, a sink that records
//! each super-block's integers, and the check that every tier equals the
//! baseline.

extern crate std;

use crate::kquant::{Sink, q4_k_row, q6_k_row};
use crate::*;
use alloc::vec;
use alloc::vec::Vec;

/// splitmix64, so the tests need no crate and repeat exactly.
pub(crate) struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [-1, 1).
    pub fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Format {
    Q4K,
    Q6K,
}

impl Format {
    pub const ALL: [Self; 2] = [Self::Q4K, Self::Q6K];

    pub fn block_bytes(self) -> usize {
        match self {
            Self::Q4K => q4_k::BLOCK_BYTES,
            Self::Q6K => q6_k::BLOCK_BYTES,
        }
    }

    pub fn row_bytes(self, len: usize) -> usize {
        len / SUPER_LEN * self.block_bytes()
    }

    pub fn matvec(self, tier: Tier, w: &[u8], x: &Q8KVec, y: &mut [f32]) {
        match self {
            Self::Q4K => q4_k_matvec(tier, w, x, y),
            Self::Q6K => q6_k_matvec(tier, w, x, y),
        }
    }

    /// Each super-block's integers for one row: (a, b) for Q4_K, (s, 0) for Q6_K.
    pub fn ints(self, tier: Tier, row: &[u8], x: &Q8KVec) -> Vec<(i32, i32)> {
        let mut ints = Ints(Vec::new());
        match self {
            Self::Q4K => q4_k_row(tier, row, x, &mut ints),
            Self::Q6K => q6_k_row(tier, row, x, &mut ints),
        }
        ints.0
    }

    pub fn dequantize(self, w: &[u8], out: &mut [f32]) {
        match self {
            Self::Q4K => q4_k::dequantize_row(w, out),
            Self::Q6K => q6_k::dequantize_row(w, out),
        }
    }

    pub fn quantize(self, x: &[f32], out: &mut [u8]) {
        match self {
            Self::Q4K => q4_k::quantize_row(x, out),
            Self::Q6K => q6_k::quantize_row(x, out),
        }
    }

    /// Byte offsets of the f16 scales in a block.
    pub fn scale_offsets(self) -> &'static [usize] {
        match self {
            Self::Q4K => &[0, 2],
            Self::Q6K => &[q6_k::D],
        }
    }
}

pub(crate) struct Ints(pub Vec<(i32, i32)>);

impl Sink for Ints {
    fn q4_k(&mut self, _: f32, _: f32, _: f32, a: i32, b: i32) {
        self.0.push((a, b));
    }

    fn q6_k(&mut self, _: f32, _: f32, s: i32) {
        self.0.push((s, 0));
    }
}

/// Rows of random bytes whose f16 scales are finite and small, so no row's total
/// is NaN or infinite and hides what its integers were.
pub(crate) fn random_rows(rng: &mut Rng, format: Format, len: usize, rows: usize) -> Vec<u8> {
    let mut w: Vec<u8> = (0..format.row_bytes(len) * rows)
        .map(|_| rng.next() as u8)
        .collect();
    for block in w.chunks_exact_mut(format.block_bytes()) {
        for &at in format.scale_offsets() {
            let scale = f16::from_f32(rng.unit() * 0.01);
            block[at..at + 2].copy_from_slice(&scale.to_le_bytes());
        }
    }
    w
}

pub(crate) fn random_x(rng: &mut Rng, len: usize, magnitude: f32) -> Q8KVec {
    let x: Vec<f32> = (0..len).map(|_| rng.unit() * magnitude).collect();
    let mut q = Q8KVec::new(len);
    q.quantize(&x);
    q
}

/// Every tier's super-block integers and products equal the baseline's, exactly.
/// The integers are compared first: above 2^24 their conversion to f32 rounds,
/// and could hide an error in the last bits. On CI the runner must offer at
/// least AVX2; every tier the CPU has is checked, never skipped.
pub(crate) fn assert_tiers_agree(format: Format, w: &[u8], x: &Q8KVec, rows: usize) {
    if std::env::var_os("CI").is_some() {
        assert!(Tier::all().count() >= 2, "only the baseline tier runs here");
    }
    let row = format.row_bytes(x.len());
    let mut base = vec![0.0f32; rows];
    format.matvec(Tier::BASELINE, w, x, &mut base);
    for tier in Tier::all().skip(1) {
        for (r, w) in w.chunks_exact(row.max(1)).take(rows).enumerate() {
            let (want, got) = (format.ints(Tier::BASELINE, w, x), format.ints(tier, w, x));
            assert_eq!(want, got, "{format:?} row {r}: {} integers", tier.name());
        }
        let mut y = vec![0.0f32; rows];
        format.matvec(tier, w, x, &mut y);
        for (r, (a, b)) in base.iter().zip(&y).enumerate() {
            let name = tier.name();
            assert_eq!(
                a.to_bits(),
                b.to_bits(),
                "{format:?} row {r}: {name} {b}, baseline {a}"
            );
        }
    }
}
