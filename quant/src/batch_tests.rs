//! The batched products: in every tier, each token's result is the bits of the
//! one-token product of the baseline, on random, saturated, all-zero and
//! non-finite blocks, for every group size; for the K-quants each token's
//! super-block integers are compared first.

extern crate std;

use crate::batch::{q4_k_n, q6_k_n};
use crate::ksupport::*;
use crate::*;
use alloc::vec;
use alloc::vec::Vec;

/// Batches long enough to give every group size, and one prefill block.
const TOKEN_COUNTS: [usize; 7] = [1, 2, 3, 4, 5, 7, 32];

#[derive(Clone, Copy, Debug)]
enum Kind {
    Q8_0,
    K(Format),
}

const KINDS: [Kind; 3] = [Kind::Q8_0, Kind::K(Format::Q4K), Kind::K(Format::Q6K)];

impl Kind {
    fn row_bytes(self, len: usize) -> usize {
        match self {
            Self::Q8_0 => crate::row_bytes(len),
            Self::K(f) => f.row_bytes(len),
        }
    }

    fn block_bytes(self) -> usize {
        match self {
            Self::Q8_0 => BLOCK_BYTES,
            Self::K(f) => f.block_bytes(),
        }
    }

    fn scale_offsets(self) -> &'static [usize] {
        match self {
            Self::Q8_0 => &[0],
            Self::K(f) => f.scale_offsets(),
        }
    }
}

/// The same activations in both formats.
struct Xs {
    q8: Vec<QVec>,
    k: Vec<Q8KVec>,
}

impl Xs {
    fn from_values(values: &[Vec<f32>]) -> Self {
        let len = values[0].len();
        let q8 = values
            .iter()
            .map(|v| {
                let mut q = QVec::new(len);
                q.quantize(v);
                q
            })
            .collect();
        let k = values
            .iter()
            .map(|v| {
                let mut q = Q8KVec::new(len);
                q.quantize(v);
                q
            })
            .collect();
        Self { q8, k }
    }

    fn random(rng: &mut Rng, len: usize, tokens: usize) -> Self {
        let values: Vec<Vec<f32>> = (0..tokens)
            .map(|t| (0..len).map(|_| rng.unit() * (1 + t % 5) as f32).collect())
            .collect();
        Self::from_values(&values)
    }

    fn matmul(&self, kind: Kind, tier: Tier, w: &[u8], y: &mut [f32]) {
        match kind {
            Kind::Q8_0 => q8_0_matmul(tier, w, &self.q8, y),
            Kind::K(Format::Q4K) => q4_k_matmul(tier, w, &self.k, y),
            Kind::K(Format::Q6K) => q6_k_matmul(tier, w, &self.k, y),
        }
    }

    fn matvec(&self, kind: Kind, tier: Tier, w: &[u8], t: usize, y: &mut [f32]) {
        match kind {
            Kind::Q8_0 => matvec(tier, w, &self.q8[t], y),
            Kind::K(f) => f.matvec(tier, w, &self.k[t], y),
        }
    }
}

/// Each token's super-block integers for one row from the batched kernel run on
/// groups of `N` tokens (the last group shorter).
fn batch_ints<const N: usize>(
    format: Format,
    tier: Tier,
    row: &[u8],
    xs: &[Q8KVec],
) -> Vec<Vec<(i32, i32)>> {
    let mut out = Vec::new();
    for group in xs.chunks(N) {
        macro_rules! run {
            ($n:literal) => {{
                let xs: &[Q8KVec; $n] = group.try_into().unwrap();
                let mut sinks: [Ints; $n] = core::array::from_fn(|_| Ints(Vec::new()));
                match format {
                    Format::Q4K => q4_k_n(tier, row, xs, &mut sinks),
                    Format::Q6K => q6_k_n(tier, row, xs, &mut sinks),
                }
                out.extend(sinks.into_iter().map(|s| s.0));
            }};
        }
        match group.len() {
            4 => run!(4),
            3 => run!(3),
            2 => run!(2),
            _ => run!(1),
        }
    }
    out
}

/// Every tier's batched product equals the baseline's one-token product of
/// every token, bit for bit, and for the K-quants each token's integers equal
/// the baseline's first. On CI the runner must offer at least AVX2; every tier
/// the CPU has is checked, never skipped.
fn assert_batches_agree(kind: Kind, w: &[u8], xs: &Xs, rows: usize) {
    if std::env::var_os("CI").is_some() {
        assert!(Tier::all().count() >= 2, "only the baseline tier runs here");
    }
    let tokens = xs.k.len();
    let row = kind.row_bytes(xs.k[0].len());
    let mut want = vec![0.0f32; rows * tokens];
    for t in 0..tokens {
        let mut y = vec![0.0f32; rows];
        xs.matvec(kind, Tier::BASELINE, w, t, &mut y);
        for (r, v) in y.into_iter().enumerate() {
            want[r * tokens + t] = v;
        }
    }
    for tier in Tier::all() {
        if let Kind::K(format) = kind {
            for (r, w) in w.chunks_exact(row.max(1)).take(rows).enumerate() {
                let base: Vec<_> =
                    xs.k.iter()
                        .map(|x| format.ints(Tier::BASELINE, w, x))
                        .collect();
                for (n, got) in [
                    batch_ints::<1>(format, tier, w, &xs.k),
                    batch_ints::<2>(format, tier, w, &xs.k),
                    batch_ints::<3>(format, tier, w, &xs.k),
                    batch_ints::<4>(format, tier, w, &xs.k),
                ]
                .into_iter()
                .enumerate()
                {
                    let name = tier.name();
                    assert_eq!(
                        base,
                        got,
                        "{format:?} row {r}: {name} integers in groups of {}",
                        n + 1
                    );
                }
            }
        }
        let mut y = vec![0.0f32; rows * tokens];
        xs.matmul(kind, tier, w, &mut y);
        for (i, (a, b)) in want.iter().zip(&y).enumerate() {
            let (r, t, name) = (i / tokens, i % tokens, tier.name());
            assert_eq!(
                a.to_bits(),
                b.to_bits(),
                "{kind:?} row {r} token {t}: {name} {b}, baseline {a}"
            );
        }
    }
}

/// Random rows whose f16 scales are finite and small.
fn random_rows(rng: &mut Rng, kind: Kind, len: usize, rows: usize) -> Vec<u8> {
    let mut w: Vec<u8> = (0..kind.row_bytes(len) * rows)
        .map(|_| rng.next() as u8)
        .collect();
    for block in w.chunks_exact_mut(kind.block_bytes()) {
        for &at in kind.scale_offsets() {
            let scale = f16::from_f32(rng.unit() * 0.01);
            block[at..at + 2].copy_from_slice(&scale.to_le_bytes());
        }
    }
    w
}

#[test]
fn batches_equal_one_token_products_on_random_rows() {
    let mut rng = Rng(31);
    for kind in KINDS {
        for (len, rows) in [(256, 40), (512, 17), (2560, 9), (9728, 3)] {
            let w = random_rows(&mut rng, kind, len, rows);
            for tokens in TOKEN_COUNTS {
                let xs = Xs::random(&mut rng, len, tokens);
                assert_batches_agree(kind, &w, &xs, rows);
            }
        }
    }
}

#[test]
fn batches_equal_one_token_products_on_saturated_and_zero_blocks() {
    let len = 512;
    // x at +-127 in every byte, at -127, all zero, and random, in one batch.
    let mut rng = Rng(32);
    let values = vec![
        (0..len)
            .map(|i| if i % 3 == 0 { -5.0 } else { 5.0 })
            .collect(),
        vec![-1.0; len],
        vec![0.0; len],
        (0..len).map(|_| rng.unit()).collect(),
        vec![1.0; len],
    ];
    let xs = Xs::from_values(&values);
    assert!(xs.k[0].qs.iter().all(|&q| q == 127 || q == -127));
    for kind in KINDS {
        let mut rows = Vec::new();
        for fill in [0xffu8, 0x00, 0x80, 0x7f, 0x55] {
            let mut row = vec![fill; kind.row_bytes(len)];
            for block in row.chunks_exact_mut(kind.block_bytes()) {
                for &at in kind.scale_offsets() {
                    block[at..at + 2].copy_from_slice(&f16::from_f32(65504.0).to_le_bytes());
                }
            }
            rows.push(row);
        }
        rows.push(vec![0; kind.row_bytes(len)]);
        let w = rows.concat();
        assert_batches_agree(kind, &w, &xs, rows.len());
    }
}

#[test]
fn batches_equal_one_token_products_on_non_finite_scales() {
    // Rows of raw random bytes: infinite, NaN and subnormal f16 scales.
    let (len, rows) = (512, 24);
    let mut rng = Rng(33);
    for kind in KINDS {
        let w: Vec<u8> = (0..kind.row_bytes(len) * rows)
            .map(|_| rng.next() as u8)
            .collect();
        let xs = Xs::random(&mut rng, len, 7);
        assert_batches_agree(kind, &w, &xs, rows);
        let mut y = vec![0.0; rows * 7];
        xs.matmul(kind, Tier::best(), &w, &mut y);
        assert!(
            y.iter().any(|v| v.is_nan()),
            "{kind:?}: some row comes to NaN"
        );
        assert!(
            y.iter()
                .all(|v| !v.is_nan() || v.to_bits() == f32::NAN.to_bits())
        );
    }
}

#[test]
fn batches_reach_the_extreme_integers_exactly() {
    // Every Q4_K code 15 with scales and minimums 63, against x at 127 and -127.
    let x_at = |v: f32| {
        let mut x = Q8KVec::new(512);
        x.quantize(&vec![v; 512]);
        x
    };
    let xs = [x_at(1.0), x_at(-1.0), x_at(1.0)];
    let mut q4 = vec![0xffu8; q4_k::row_bytes(512)];
    for block in q4.as_chunks_mut::<{ q4_k::BLOCK_BYTES }>().0 {
        block[..4].copy_from_slice(&[0, 0x3c, 0, 0x3c]);
    }
    let (a, b) = (30_723_840, 2_048_256);
    let mut q6 = vec![0u8; q6_k::row_bytes(512)];
    for block in q6.as_chunks_mut::<{ q6_k::BLOCK_BYTES }>().0 {
        block[q6_k::SCALES..q6_k::D].fill(0x80);
        block[q6_k::D..].copy_from_slice(&[0, 0x3c]);
    }
    for tier in Tier::all() {
        let q4_ints = batch_ints::<4>(Format::Q4K, tier, &q4, &xs);
        assert_eq!(
            q4_ints,
            [vec![(a, b); 2], vec![(-a, -b); 2], vec![(a, b); 2]]
        );
        let q6_ints = batch_ints::<4>(Format::Q6K, tier, &q6, &xs);
        assert_eq!(q6_ints[0], [(133_169_152, 0); 2], "{}", tier.name());
    }
}

#[test]
fn matmul_shapes_are_checked() {
    let xs = vec![Q8KVec::new(256); 3];
    let w = vec![0u8; q4_k::row_bytes(256) * 2];
    let mut y = vec![1.0f32; 6];
    q4_k_matmul(Tier::best(), &w, &xs, &mut y);
    assert_eq!(y, [0.0; 6]);
    let wrong = std::panic::catch_unwind(|| {
        let mut y = vec![0.0f32; 9];
        q4_k_matmul(Tier::best(), &w, &xs, &mut y);
    });
    assert!(
        wrong.is_err(),
        "three rows of output for two rows of weights"
    );
    let ragged = std::panic::catch_unwind(|| {
        let xs = [Q8KVec::new(256), Q8KVec::new(512)];
        let mut y = vec![0.0f32; 4];
        q6_k_matmul(Tier::best(), &w, &xs, &mut y);
    });
    assert!(ragged.is_err(), "vectors of different lengths");
}
