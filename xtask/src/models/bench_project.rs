//! The decode-step projection of `bench quant`: for each catalog model, every
//! matrix its decode list names, allocated apart and filled with blocks quantised
//! from Gaussian weights (so scales are as small, and as often subnormal, as in a
//! real file), multiplied once per token in the groups the engine will use, one
//! broadcast per group. What the matrix products leave out is printed beside it.

use super::catalog::Catalog;
use super::decode::{Decode, Mat};
use gguf::TensorType;
use quant::{Parallel, Product, Q8KVec, QVec, Tier, matvec_parallel};
use std::time::Duration;

/// The KV cache's reach in these figures: the engine's context cap (kabi's
/// CONTEXT_CAP), at f32 as the L1-L4 engine keeps it.
const KV_POSITIONS: u64 = 512;

/// Memory read GB/s by thread count.
pub struct Stream {
    pub gbps: Vec<(usize, f64)>,
}

impl Stream {
    pub fn at(&self, threads: usize) -> f64 {
        self.gbps
            .iter()
            .find(|(n, _)| *n == threads)
            .map_or(f64::NAN, |(_, g)| *g)
    }
}

pub fn run(threads: &[usize], pools: &[Box<dyn Parallel>], stream: &Stream) -> Result<(), String> {
    let catalog = Catalog::load()?;
    let tiers: Vec<Tier> = [Tier::all().nth(1), Some(Tier::best())]
        .into_iter()
        .flatten()
        .fold(Vec::new(), |mut v, t| {
            if !v.contains(&t) {
                v.push(t);
            }
            v
        });
    println!(
        "\nDecode steps, matrix products only: every matrix once per token, in the \
         engine's groups ([q k v] [o] [gate up] [down], then the output), on this \
         host's threads. Tokens per second:"
    );
    let head: Vec<String> = threads.iter().map(|n| format!("{n:>3} thr")).collect();
    println!(
        "  {:<32}{:<12}{}  read rate",
        "model, bytes per token",
        "tier",
        head.join("  ")
    );
    for entry in &catalog.models {
        let mut step = Step::new(&entry.decode);
        let label = format!("{} {:.2} GB", entry.id, step.bytes as f64 / 1e9);
        let mut rates = Vec::new();
        for &tier in &tiers {
            let cells: Vec<String> = threads
                .iter()
                .zip(pools)
                .map(|(&n, pool)| {
                    let times = super::bench::sample(|| step.pass(&**pool, tier));
                    rates.push((tier, n, times[0]));
                    format!("{:7.1}", 1.0 / times[0].as_secs_f64())
                })
                .collect();
            let ceiling = stream.at(*threads.last().unwrap_or(&1)) * 1e9 / step.bytes as f64;
            println!(
                "  {label:<32}{:<12}{}  {ceiling:5.1}",
                tier.name(),
                cells.join("  ")
            );
        }
        let kv = step.kv_bytes(KV_POSITIONS);
        let with_kv: Vec<String> = threads
            .iter()
            .map(|&n| {
                let pass = best_time(&rates, n);
                let kv_time = kv as f64 / (stream.at(n) * 1e9);
                format!("{n}: {:.1}", 1.0 / (pass.as_secs_f64() + kv_time))
            })
            .collect();
        println!(
            "    estimate, not measured: adding the f32 KV cache's {} MB a token at \
             {KV_POSITIONS} positions at the read rate: {} tokens/s (attention does more \
             than read it)",
            kv / 1_000_000,
            with_kv.join(", ")
        );
    }
    println!(
        "  read rate: the memory read rate on the most threads over the bytes a token \
         reads; the products' dynamic split can beat the read test's even one by up to \
         a tenth. 16 threads oversubscribe the 16 logical CPUs while the host pool's \
         caller spins. Host-native threads and 4 KiB pages; a VM adds nested paging \
         and exits on wake-ups, and CI or TCG numbers are no reference."
    );
    Ok(())
}

/// The fastest tier's time for one token on `threads` threads.
fn best_time(rates: &[(Tier, usize, Duration)], threads: usize) -> Duration {
    rates
        .iter()
        .filter(|(_, n, _)| *n == threads)
        .map(|(_, _, t)| *t)
        .min()
        .unwrap_or(Duration::MAX)
}

/// A matrix's weights and its outputs.
struct Weights {
    mat: Mat,
    data: Vec<u8>,
    y: Vec<f32>,
}

/// Matrices that share an input, the input, and its quantised forms.
struct Group {
    weights: Vec<Weights>,
    x: Vec<f32>,
    q8: Option<QVec>,
    k: Option<Q8KVec>,
}

/// One decode step's matrices, group by group, each with its own memory.
struct Step {
    groups: Vec<Group>,
    bytes: u64,
    kv_dim: u64,
    layers: u64,
}

impl Step {
    fn new(decode: &Decode) -> Self {
        let mut groups = Vec::new();
        let mut bytes = 0;
        let mut add = |mats: &[Mat]| {
            let cols = mats[0].cols as usize;
            let weights: Vec<Weights> = mats
                .iter()
                .map(|m| Weights {
                    mat: m.clone(),
                    data: weights(m.ty, m.cols as usize, m.rows as usize),
                    y: vec![0.0; m.rows as usize],
                })
                .collect();
            bytes += weights.iter().map(|w| w.data.len() as u64).sum::<u64>();
            let q8 = mats.iter().any(|m| m.ty == TensorType::Q8_0);
            let k = mats.iter().any(|m| m.ty != TensorType::Q8_0);
            groups.push(Group {
                weights,
                x: activation(cols),
                q8: q8.then(|| QVec::new(cols)),
                k: k.then(|| Q8KVec::new(cols)),
            });
        };
        for layers in &decode.layers {
            for _ in 0..layers.count {
                for group in &layers.groups {
                    add(group);
                }
            }
        }
        add(std::slice::from_ref(&decode.output));
        let kv_dim = decode.layers[0].groups[0][1].rows;
        let layers = decode.layers.iter().map(|l| l.count).sum();
        Self {
            groups,
            bytes,
            kv_dim,
            layers,
        }
    }

    /// Keys and values for every layer at `positions` positions, as f32.
    fn kv_bytes(&self, positions: u64) -> u64 {
        2 * self.layers * self.kv_dim * 4 * positions
    }

    /// One token: each group's input quantised, then its matrices in one
    /// broadcast.
    fn pass(&mut self, pool: &dyn Parallel, tier: Tier) {
        for group in &mut self.groups {
            let Group { weights, x, q8, k } = group;
            if let Some(q8) = q8.as_mut() {
                q8.quantize(x);
            }
            if let Some(k) = k.as_mut() {
                k.quantize(x);
            }
            let (q8, k) = (q8.as_slice(), k.as_slice());
            let mut items: Vec<(Product<'_>, &mut [f32])> = weights
                .iter_mut()
                .map(|w| (product(&w.mat, &w.data, q8, k), w.y.as_mut_slice()))
                .collect();
            matvec_parallel(pool, tier, &mut items);
        }
    }
}

fn product<'a>(mat: &Mat, data: &'a [u8], q8: &'a [QVec], k: &'a [Q8KVec]) -> Product<'a> {
    let rows = mat.rows as usize;
    match mat.ty {
        TensorType::Q8_0 => Product::q8_0(data, rows, q8),
        TensorType::Q4_K => Product::q4_k(data, rows, k),
        _ => Product::q6_k(data, rows, k),
    }
}

pub fn type_named(name: &str) -> TensorType {
    match name {
        "Q8_0" => TensorType::Q8_0,
        "Q4_K" => TensorType::Q4_K,
        _ => TensorType::Q6_K,
    }
}

pub fn row_bytes(ty: TensorType, cols: usize) -> usize {
    cols / ty.block_len() as usize * ty.block_bytes() as usize
}

/// `rows` rows of `cols` values of type `ty`: a few MB of blocks quantised from
/// Gaussian weights (sigma 0.02, as in trained matrices), repeated. Every byte is
/// written, so no page is left for the OS to fill with its shared zero page.
pub fn weights(ty: TensorType, cols: usize, rows: usize) -> Vec<u8> {
    let pattern = pattern(ty);
    let mut data = vec![0u8; rows * row_bytes(ty, cols)];
    for chunk in data.chunks_mut(pattern.len()) {
        chunk.copy_from_slice(&pattern[..chunk.len()]);
    }
    data
}

fn pattern(ty: TensorType) -> Vec<u8> {
    const VALUES: usize = 1 << 21;
    let x = gaussian(VALUES, 0.02, 7);
    let mut out = vec![0u8; row_bytes(ty, VALUES)];
    match ty {
        TensorType::Q8_0 => quant::quantize_row(&x, &mut out),
        TensorType::Q4_K => quant::q4_k::quantize_row(&x, &mut out),
        _ => quant::q6_k::quantize_row(&x, &mut out),
    }
    out
}

/// An activation vector of `len` values, Gaussian with sigma 1.
pub fn activation(len: usize) -> Vec<f32> {
    gaussian(len, 1.0, len as u64)
}

/// Box-Muller over xorshift64*.
fn gaussian(len: usize, sigma: f32, seed: u64) -> Vec<f32> {
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    let mut unit = || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        ((state.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32 + 0.5) / (1u64 << 24) as f32
    };
    (0..len)
        .map(|_| {
            let (u, v) = (unit(), unit());
            sigma * (-2.0 * u.ln()).sqrt() * (std::f32::consts::TAU * v).cos()
        })
        .collect()
}
