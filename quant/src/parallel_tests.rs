//! Row ranges and parallel products: every split, every pool, every tier gives
//! the single-thread bits.

extern crate std;

use crate::ksupport::{Format, Rng, random_rows, random_x};
use crate::parallel::{chunk_rows, locate};
use crate::*;
use alloc::vec;
use alloc::vec::Vec;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Workers on std threads, started for each broadcast.
struct Threads(usize);

impl Parallel for Threads {
    fn workers(&self) -> usize {
        self.0
    }

    fn broadcast(&self, job: &(dyn Fn(usize, usize) + Sync)) {
        std::thread::scope(|s| {
            for worker in 0..self.0 {
                s.spawn(move || job(worker, self.0));
            }
        });
    }
}

/// Pools that break the contract in ways a safe pool can: they say any number
/// of workers, run the job on fewer of them, or on some twice.
enum Odd {
    Skips,
    Twice,
    Claims(usize),
}

impl Parallel for Odd {
    fn workers(&self) -> usize {
        match self {
            Self::Skips | Self::Twice => 4,
            Self::Claims(n) => *n,
        }
    }

    fn broadcast(&self, job: &(dyn Fn(usize, usize) + Sync)) {
        match self {
            Self::Skips => {}
            Self::Twice => std::thread::scope(|s| {
                for worker in [0, 0, 1, 1] {
                    s.spawn(move || job(worker, 4));
                }
            }),
            Self::Claims(_) => job(0, 1),
        }
    }
}

/// Weights and vectors of one format.
struct Case {
    format: Option<Format>,
    w: Vec<u8>,
    q8: Vec<QVec>,
    k: Vec<Q8KVec>,
    rows: usize,
}

impl Case {
    /// `rows` rows of `cols` values, times `tokens` random vectors; `format`
    /// None is Q8_0.
    fn new(rng: &mut Rng, format: Option<Format>, cols: usize, rows: usize, tokens: usize) -> Self {
        let xs: Vec<Vec<f32>> = (0..tokens)
            .map(|_| (0..cols).map(|_| rng.unit() * 2.0).collect())
            .collect();
        let (w, q8, k) = match format {
            Some(format) => {
                let k = (0..tokens).map(|_| random_x(rng, cols, 2.0)).collect();
                (random_rows(rng, format, cols, rows), Vec::new(), k)
            }
            None => {
                let w: Vec<u8> = (0..rows * row_bytes(cols))
                    .map(|_| rng.next() as u8)
                    .collect();
                let mut w = w;
                for block in w.as_chunks_mut::<BLOCK_BYTES>().0 {
                    block[..2].copy_from_slice(&f16::from_f32(rng.unit() * 0.05).to_le_bytes());
                }
                let q8 = xs
                    .iter()
                    .map(|x| {
                        let mut q = QVec::new(cols);
                        q.quantize(x);
                        q
                    })
                    .collect();
                (w, q8, Vec::new())
            }
        };
        Self {
            format,
            w,
            q8,
            k,
            rows,
        }
    }

    fn product(&self) -> Product<'_> {
        match self.format {
            None => Product::q8_0(&self.w, self.rows, &self.q8),
            Some(Format::Q4K) => Product::q4_k(&self.w, self.rows, &self.k),
            Some(Format::Q6K) => Product::q6_k(&self.w, self.rows, &self.k),
        }
    }
}

fn bits(y: &[f32]) -> Vec<u32> {
    y.iter().map(|v| v.to_bits()).collect()
}

const FORMATS: [Option<Format>; 3] = [None, Some(Format::Q4K), Some(Format::Q6K)];

#[test]
fn chunks_tile_every_product_exactly() {
    for workers in 1..=64 {
        for total in 0..=300 {
            let chunk = chunk_rows(total, workers);
            assert!(chunk >= 16 && chunk.is_multiple_of(16));
            // Split the rows over three products and walk every chunk.
            let rows = [total / 3, total / 2, total - total / 3 - total / 2];
            let mut rng = Rng(total as u64);
            let cases: Vec<Case> = rows
                .iter()
                .map(|&r| Case::new(&mut rng, Some(Format::Q4K), 256, r, 1))
                .collect();
            let products: Vec<Option<Product>> = cases.iter().map(|c| Some(c.product())).collect();
            let count: usize = rows.iter().map(|r| r.div_ceil(chunk)).sum();
            let mut seen = vec![vec![false; total]; 3];
            for c in 0..count {
                let (i, range) = locate(&products, c, chunk);
                assert!(!range.is_empty() && range.end <= rows[i]);
                for r in range {
                    assert!(!seen[i][r], "row {r} of product {i} twice");
                    seen[i][r] = true;
                }
            }
            // About four chunks a worker once there are rows enough, never one
            // chunk doing all the work.
            if total >= 64 * workers {
                let most = 4 * workers + rows.len();
                assert!((2 * workers..=most).contains(&count), "{count} chunks");
            }
            for (i, &r) in rows.iter().enumerate() {
                assert!(
                    seen[i][..r].iter().all(|&s| s),
                    "{workers} workers, {total} rows"
                );
            }
        }
    }
}

#[test]
fn parallel_equals_one_thread_for_every_pool_format_and_tier() {
    let mut rng = Rng(41);
    for format in FORMATS {
        for (rows, tokens) in [(0, 1), (1, 1), (17, 3), (1000, 1), (203, 2)] {
            let case = Case::new(&mut rng, format, 512, rows, tokens);
            let product = case.product();
            for tier in Tier::all() {
                let mut want = vec![0.0f32; rows * tokens];
                product.run(tier, 0..rows, &mut want);
                let pools: [&dyn Parallel; 8] = [
                    &Serial,
                    &Threads(1),
                    &Threads(3),
                    &Threads(8),
                    &Odd::Skips,
                    &Odd::Twice,
                    &Odd::Claims(0),
                    &Odd::Claims(1000),
                ];
                for (p, par) in pools.into_iter().enumerate() {
                    let mut y = vec![f32::NAN; rows * tokens];
                    let left = matvec_parallel(par, tier, &mut [(product, &mut y)]);
                    assert_eq!(
                        bits(&y),
                        bits(&want),
                        "{format:?} {} rows {rows}",
                        tier.name()
                    );
                    // Only the pool that never runs the job leaves work behind.
                    assert_eq!(left > 0, p == 4 && rows > 0, "pool {p}, {rows} rows");
                }
            }
        }
    }
}

#[test]
fn row_ranges_tokens_and_fused_products_agree() {
    let mut rng = Rng(43);
    for format in FORMATS {
        let case = Case::new(&mut rng, format, 768, 40, 3);
        let product = case.product();
        let tier = Tier::best();
        let mut all = vec![0.0f32; 40 * 3];
        product.run(tier, 0..40, &mut all);
        // Any range of rows is the same slice of the whole.
        for range in [0..0, 0..1, 5..17, 39..40] {
            let mut part = vec![0.0f32; range.len() * 3];
            product.run(tier, range.clone(), &mut part);
            assert_eq!(bits(&part), bits(&all[range.start * 3..range.end * 3]));
        }
        // A batch of tokens is each token alone, laid out [row][token].
        for t in 0..3 {
            let single = match format {
                None => Product::q8_0(&case.w, 40, &case.q8[t..t + 1]),
                Some(Format::Q4K) => Product::q4_k(&case.w, 40, &case.k[t..t + 1]),
                Some(Format::Q6K) => Product::q6_k(&case.w, 40, &case.k[t..t + 1]),
            };
            let mut y = vec![0.0f32; 40];
            single.run(tier, 0..40, &mut y);
            let column: Vec<f32> = all.iter().skip(t).step_by(3).copied().collect();
            assert_eq!(bits(&y), bits(&column), "{format:?} token {t}");
        }
    }
    // q, k and v of different formats in one broadcast, as each alone.
    let cases: Vec<Case> = FORMATS
        .iter()
        .enumerate()
        .map(|(i, &f)| Case::new(&mut rng, f, 1024, 50 + 13 * i, 1))
        .collect();
    let mut alone: Vec<Vec<f32>> = Vec::new();
    for case in &cases {
        let mut y = vec![0.0; case.rows];
        case.product().run(Tier::BASELINE, 0..case.rows, &mut y);
        alone.push(y);
    }
    let mut outs: Vec<Vec<f32>> = cases.iter().map(|c| vec![0.0; c.rows]).collect();
    let mut items: Vec<(Product, &mut [f32])> = cases
        .iter()
        .zip(outs.iter_mut())
        .map(|(c, y)| (c.product(), y.as_mut_slice()))
        .collect();
    matvec_parallel(&Threads(4), Tier::BASELINE, &mut items);
    for (got, want) in outs.iter().zip(&alone) {
        assert_eq!(bits(got), bits(want));
    }
}

#[test]
fn a_pool_runs_jobs_on_its_workers() {
    // The workers, not the caller, do every chunk; and a big product is cut into
    // at least a chunk per worker.
    let mut rng = Rng(45);
    let case = Case::new(&mut rng, Some(Format::Q6K), 2560, 4096, 1);
    let calls = AtomicUsize::new(0);
    struct Counting<'a>(&'a AtomicUsize);
    impl Parallel for Counting<'_> {
        fn workers(&self) -> usize {
            8
        }
        fn broadcast(&self, job: &(dyn Fn(usize, usize) + Sync)) {
            std::thread::scope(|s| {
                for worker in 0..8 {
                    s.spawn(move || {
                        self.0.fetch_add(1, Ordering::Relaxed);
                        job(worker, 8);
                    });
                }
            });
        }
    }
    let mut y = vec![0.0f32; 4096];
    let item = (case.product(), y.as_mut_slice());
    let left = matvec_parallel(&Counting(&calls), Tier::best(), &mut [item]);
    assert_eq!((left, calls.load(Ordering::Relaxed)), (0, 8));
    assert!(4096usize.div_ceil(chunk_rows(4096, 8)) >= 8);
}

#[test]
fn zero_columns_give_zeros() {
    let x = [Q8KVec::new(0), Q8KVec::new(0)];
    let product = Product::q4_k(&[], 5, &x);
    let mut y = vec![1.0f32; 10];
    product.run(Tier::best(), 0..5, &mut y);
    assert!(y.iter().all(|&v| v == 0.0));
    y.fill(1.0);
    matvec_parallel(&Threads(2), Tier::best(), &mut [(product, &mut y)]);
    assert!(y.iter().all(|&v| v == 0.0));
}

#[test]
#[should_panic(expected = "rows of 144 bytes")]
fn weights_of_the_wrong_size_are_refused() {
    let x = [Q8KVec::new(256)];
    Product::q4_k(&[0; 143], 1, &x);
}

#[test]
#[should_panic(expected = "different lengths")]
fn vectors_of_different_lengths_are_refused() {
    let x = [Q8KVec::new(256), Q8KVec::new(512)];
    Product::q6_k(&[0; 210], 1, &x);
}

#[test]
#[should_panic(expected = "at least one vector")]
fn a_product_needs_a_vector() {
    Product::q8_0(&[], 0, &[]);
}

#[test]
#[should_panic(expected = "rows 2..4")]
fn rows_past_the_matrix_are_refused() {
    let x = [Q8KVec::new(256)];
    let mut y = [0.0; 2];
    Product::q4_k(&[0; 3 * 144], 3, &x).run(Tier::BASELINE, 2..4, &mut y);
}

#[test]
#[should_panic(expected = "output size")]
fn an_output_of_the_wrong_size_is_refused() {
    let x = [Q8KVec::new(256)];
    let mut y = [0.0; 2];
    let product = Product::q4_k(&[0; 3 * 144], 3, &x);
    matvec_parallel(&Serial, Tier::BASELINE, &mut [(product, &mut y)]);
}
