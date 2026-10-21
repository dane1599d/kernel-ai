//! `bench quant`'s prefill table: one prompt block of [`infer::PREFILL_TOKENS`]
//! tokens through Qwen3-4B's layers, matrix products only, the old way (the
//! one-token kernel once per row and token, as `Product::run` ran batches before
//! the batched kernels) and with the batched kernels. Each distinct layer of the
//! catalog's decode list is built once on generated weights of its real types and
//! shapes, timed, and counted as often as the model repeats it; prompt tokens
//! skip the output projection, so it is left out.

use super::bench::sample;
use super::bench_project::{activation, weights};
use super::catalog::Catalog;
use super::decode::Mat;
use gguf::TensorType;
use quant::{Parallel, Product, Q8KVec, QVec, Tier, for_each_chunk, matvec_parallel};

const MODEL: &str = "qwen3-4b-2507";
const TOKENS: usize = infer::PREFILL_TOKENS;
/// Rows per piece on the old path, as `matvec_parallel` cuts them at least.
const PIECE_ROWS: usize = 16;

pub fn run(threads: &[usize], pools: &[Box<dyn Parallel>]) -> Result<(), String> {
    let catalog = Catalog::load()?;
    let entry = catalog
        .models
        .iter()
        .find(|m| m.id == MODEL)
        .ok_or_else(|| format!("{MODEL} is not in the catalog"))?;
    let mut layers: Vec<(f64, Vec<Group>)> = entry
        .decode
        .layers
        .iter()
        .map(|l| {
            (
                l.count as f64,
                l.groups.iter().map(|g| Group::new(g)).collect(),
            )
        })
        .collect();
    println!(
        "\nPrefill, matrix products only: {MODEL}'s layers for one {TOKENS}-token prompt \
         block on this host's threads (batched: each group of matrices that share an \
         input in one broadcast; one-token: a broadcast per matrix). Tokens per second, \
         from the median run of each distinct layer:"
    );
    let head: Vec<String> = threads.iter().map(|n| format!("{n:>3} thr")).collect();
    println!("  {:<38}{:<13}{}", "kernel", "tier", head.join("  "));
    let mut rows: Vec<(&str, Tier, bool)> =
        vec![("one-token kernel per token", Tier::best(), false)];
    // The baseline's batched kernel runs at a few tokens a second; it is timed
    // only where no SIMD tier runs.
    let simd: Vec<Tier> = Tier::all().skip(1).collect();
    let batched = if simd.is_empty() {
        vec![Tier::BASELINE]
    } else {
        simd
    };
    rows.extend(batched.into_iter().map(|t| ("batched", t, true)));
    for (label, tier, batched) in rows {
        let cells: Vec<String> = threads
            .iter()
            .zip(pools)
            .map(|(_, pool)| {
                let seconds: f64 = layers
                    .iter_mut()
                    .map(|(count, groups)| {
                        let time = sample(|| {
                            for g in groups.iter_mut() {
                                g.run(&**pool, tier, batched);
                            }
                        })[0];
                        *count * time.as_secs_f64()
                    })
                    .sum();
                format!("{:7.1}", TOKENS as f64 / seconds)
            })
            .collect();
        let label = if batched {
            format!("{label}, {} tokens a kernel call", quant::GROUP)
        } else {
            label.into()
        };
        println!("  {label:<38}{:<13}{}", tier.name(), cells.join("  "));
    }
    Ok(())
}

/// Matrices that share an input, their outputs, and every token's quantised
/// input.
struct Group {
    mats: Vec<(Mat, Vec<u8>)>,
    outs: Vec<Vec<f32>>,
    q8: Vec<QVec>,
    k: Vec<Q8KVec>,
}

impl Group {
    fn new(mats: &[Mat]) -> Self {
        let cols = mats[0].cols as usize;
        let xs: Vec<Vec<f32>> = (0..TOKENS)
            .map(|t| {
                let mut x = activation(cols);
                x.rotate_left(t * 7 % cols);
                x
            })
            .collect();
        Self {
            mats: mats
                .iter()
                .map(|m| (m.clone(), weights(m.ty, cols, m.rows as usize)))
                .collect(),
            outs: mats
                .iter()
                .map(|m| vec![0.0; m.rows as usize * TOKENS])
                .collect(),
            q8: xs
                .iter()
                .map(|x| {
                    let mut v = QVec::new(cols);
                    v.quantize(x);
                    v
                })
                .collect(),
            k: xs
                .iter()
                .map(|x| {
                    let mut v = Q8KVec::new(cols);
                    v.quantize(x);
                    v
                })
                .collect(),
        }
    }

    /// Every matrix of the group times every token, batched in one broadcast, or
    /// the old way: one broadcast per matrix, each row times each token with the
    /// one-token kernel.
    fn run(&mut self, pool: &dyn Parallel, tier: Tier, batched: bool) {
        let Self { mats, outs, q8, k } = self;
        if batched {
            let mut items: Vec<(Product<'_>, &mut [f32])> = mats
                .iter()
                .zip(outs.iter_mut())
                .map(|((m, w), y)| (product(m, w, q8, k), y.as_mut_slice()))
                .collect();
            matvec_parallel(pool, tier, &mut items);
            return;
        }
        for ((m, w), y) in mats.iter().zip(outs.iter_mut()) {
            let row = w.len() / m.rows as usize;
            for_each_chunk(pool, y, PIECE_ROWS * TOKENS, &|i, piece| {
                let rows = w[i * PIECE_ROWS * row..].chunks_exact(row);
                for (w, y) in rows.zip(piece.as_chunks_mut::<TOKENS>().0) {
                    for (t, y) in y.iter_mut().enumerate() {
                        let one = core::slice::from_mut(y);
                        match m.ty {
                            TensorType::Q8_0 => quant::matvec(tier, w, &q8[t], one),
                            TensorType::Q4_K => quant::q4_k_matvec(tier, w, &k[t], one),
                            _ => quant::q6_k_matvec(tier, w, &k[t], one),
                        }
                    }
                }
            });
        }
    }
}

fn product<'a>(m: &Mat, w: &'a [u8], q8: &'a [QVec], k: &'a [Q8KVec]) -> Product<'a> {
    let rows = w.len() / super::bench_project::row_bytes(m.ty, m.cols as usize);
    match m.ty {
        TensorType::Q8_0 => Product::q8_0(w, rows, q8),
        TensorType::Q4_K => Product::q4_k(w, rows, k),
        _ => Product::q6_k(w, rows, k),
    }
}
