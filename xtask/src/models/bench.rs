//! `cargo xtask bench quant`: how fast this PC runs the engine's kernels,
//! measured rather than estimated, and what that means for each catalog model's
//! decode step. It times memory reads (the roofline), every kernel tier on
//! weights far larger than the caches, the thread pool's cost per product, and
//! then whole decode steps of every catalog model on generated weights of the
//! model's real types and shapes, so nothing is downloaded, and one prompt
//! block of the 4B's products with the one-token and the batched kernels
//! (`bench_prefill`). Every figure is the
//! median of several runs after a warm-up; the read and one-thread kernel figures
//! show the range beside it.

use super::bench_project::{self, Stream};
use super::hostpool::HostPool;
use quant::{Parallel, Product, Q8KVec, QVec, Serial, Tier, matvec_parallel};
use std::time::{Duration, Instant};

/// Runs per figure, after one warm-up run.
const RUNS: usize = 5;
/// The memory-read buffer and each kernel's weights: far past the 32 MB of L3
/// on the 7700X, so every figure comes from DRAM.
const READ_BYTES: usize = 1 << 30;
const WEIGHT_BYTES: usize = 256 << 20;

pub fn run() -> Result<(), String> {
    let cpu = cpufeat::Features::detect();
    let logical = std::thread::available_parallelism().map_or(1, |n| n.get());
    let threads: Vec<usize> = [1, 2, 4, 8, 16]
        .into_iter()
        .filter(|&n| n <= logical)
        .collect();
    let tiers: Vec<Tier> = Tier::all().collect();
    let best = Tier::best();
    println!("xtask: {}, {logical} logical CPUs", cpu_name());
    let sets: Vec<&str> = cpu.usable().collect();
    let names: Vec<&str> = tiers.iter().map(|t| t.name()).collect();
    println!(
        "xtask: vector sets {}; kernel tiers {}",
        sets.join(" "),
        names.join(", ")
    );
    println!(
        "xtask: {} build; the engine crates are optimised either way. Figures are \
         medians of {RUNS} runs after a warm-up; [min-max] where shown.",
        if cfg!(debug_assertions) {
            "dev"
        } else {
            "release"
        }
    );
    // One thread is the caller alone, as the engine runs today; more are a
    // persistent pool like the guest's.
    let pools: Vec<Box<dyn Parallel>> = threads
        .iter()
        .map(|&n| match n {
            1 => Box::new(Serial) as Box<dyn Parallel>,
            n => Box::new(HostPool::new(n)),
        })
        .collect();
    let stream = read_table(&threads, &pools, best);
    {
        let benches = Bench::all();
        kernel_table(&benches, &tiers);
        scaling_table(&benches, &threads, &pools, best, &stream);
    }
    sync_line(&threads, &pools);
    bench_project::run(&threads, &pools, &stream)?;
    super::bench_prefill::run(&threads, &pools)?;
    calibrate(best)?;
    super::bench_4b::run(&threads, &pools, best)
}

/// Median, min and max of `RUNS` timed calls after a warm-up call.
pub fn sample(mut f: impl FnMut()) -> [Duration; 3] {
    f();
    let mut times: Vec<Duration> = (0..RUNS)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .collect();
    times.sort_unstable();
    [times[RUNS / 2], times[0], times[RUNS - 1]]
}

/// GB/s for `bytes` in each of the three times, as "median [min-max]" text
/// with the median alone.
fn rate(bytes: usize, times: [Duration; 3]) -> (f64, String) {
    let gbps = |t: Duration| bytes as f64 / t.as_secs_f64() / 1e9;
    let median = gbps(times[0]);
    (
        median,
        format!("{median:5.1} [{:.1}-{:.1}]", gbps(times[2]), gbps(times[1])),
    )
}

fn read_table(threads: &[usize], pools: &[Box<dyn Parallel>], tier: Tier) -> Stream {
    println!(
        "\nMemory read, {} MiB, each thread reading its own part:",
        READ_BYTES >> 20
    );
    let buffer = vec![1u8; READ_BYTES];
    let mut gbps = Vec::new();
    for (&n, pool) in threads.iter().zip(pools) {
        let part = READ_BYTES / n;
        let times = sample(|| {
            pool.broadcast(&|worker, _| {
                let sum = quant::stream::read(tier, &buffer[worker * part..(worker + 1) * part]);
                std::hint::black_box(sum);
            })
        });
        let (median, text) = rate(part * n, times);
        println!("  {n:>2} threads  {text} GB/s");
        gbps.push((n, median));
    }
    Stream { gbps }
}

/// One matrix of `cols`-value rows filling about WEIGHT_BYTES, and its product
/// with a random vector.
struct Bench {
    name: &'static str,
    cols: usize,
    rows: usize,
    weights: Vec<u8>,
    q8: Vec<QVec>,
    k: Vec<Q8KVec>,
}

impl Bench {
    fn all() -> Vec<Self> {
        [("Q8_0", 1024), ("Q4_K", 2560), ("Q6_K", 9728)]
            .into_iter()
            .map(|(name, cols)| {
                let ty = bench_project::type_named(name);
                let row = bench_project::row_bytes(ty, cols);
                let rows = WEIGHT_BYTES / row;
                let weights = bench_project::weights(ty, cols, rows);
                let x = bench_project::activation(cols);
                let mut q8 = QVec::new(cols);
                q8.quantize(&x);
                let mut k = Q8KVec::new(cols);
                k.quantize(&x);
                Self {
                    name,
                    cols,
                    rows,
                    weights,
                    q8: vec![q8],
                    k: vec![k],
                }
            })
            .collect()
    }

    fn product(&self) -> Product<'_> {
        match self.name {
            "Q8_0" => Product::q8_0(&self.weights, self.rows, &self.q8),
            "Q4_K" => Product::q4_k(&self.weights, self.rows, &self.k),
            _ => Product::q6_k(&self.weights, self.rows, &self.k),
        }
    }
}

fn kernel_table(benches: &[Bench], tiers: &[Tier]) {
    println!(
        "\nMatrix-vector products, one thread, {} MiB of weights, GB/s of weights read:",
        WEIGHT_BYTES >> 20
    );
    let head: Vec<String> = tiers.iter().map(|t| format!("{:>20}", t.name())).collect();
    println!("  {:<18}{}", "", head.join(""));
    for bench in benches {
        let mut y = vec![0.0f32; bench.rows];
        let cells: Vec<String> = tiers
            .iter()
            .map(|&tier| {
                let product = bench.product();
                let times = sample(|| product.run(tier, 0..bench.rows, &mut y));
                format!("{:>20}", rate(bench.weights.len(), times).1)
            })
            .collect();
        let shape = format!("{} {}x{}", bench.name, bench.cols, bench.rows);
        println!("  {shape:<18}{}", cells.join(""));
    }
}

fn scaling_table(
    benches: &[Bench],
    threads: &[usize],
    pools: &[Box<dyn Parallel>],
    tier: Tier,
    stream: &Stream,
) {
    println!(
        "\nThe {} tier on 1 to {} threads, GB/s and share of the memory read rate:",
        tier.name(),
        threads.last().unwrap_or(&1)
    );
    for bench in benches {
        let mut y = vec![0.0f32; bench.rows];
        let cells: Vec<String> = threads
            .iter()
            .zip(pools)
            .map(|(&n, pool)| {
                let product = bench.product();
                let times = sample(|| {
                    matvec_parallel(&**pool, tier, &mut [(product, &mut y)]);
                });
                let (gbps, _) = rate(bench.weights.len(), times);
                format!("{n:>3}: {gbps:5.1} ({:3.0}%)", 100.0 * gbps / stream.at(n))
            })
            .collect();
        println!("  {:<6}{}", bench.name, cells.join("  "));
    }
}

fn sync_line(threads: &[usize], pools: &[Box<dyn Parallel>]) {
    let cells: Vec<String> = threads
        .iter()
        .zip(pools)
        .map(|(&n, pool)| {
            let mut times: Vec<Duration> = (0..10_000)
                .map(|_| {
                    let start = Instant::now();
                    pool.broadcast(&|_, _| {});
                    start.elapsed()
                })
                .collect();
            times.sort_unstable();
            format!("{n}: {:.1} us", times[times.len() / 2].as_secs_f64() * 1e6)
        })
        .collect();
    println!(
        "\nThread pool, one empty broadcast by thread count (median of 10,000; one \
         thread is the caller alone; host threads, so a VM's wake-ups cost more): {}",
        cells.join(", ")
    );
}

/// The forward pass of a generated model with Qwen3-0.6B's shapes, one thread,
/// to set beside the decode-step projection for the same shapes.
fn calibrate(tier: Tier) -> Result<(), String> {
    const TOKENS: usize = 8;
    let bytes = infer::fixture::build(&infer::fixture::Spec::qwen3_0_6b());
    let file = gguf::Gguf::parse(&bytes).map_err(|e| e.to_string())?;
    let mut model = infer::Model::new(&file, tier, 64).map_err(|e| e.to_string())?;
    let mut kv = model.kv_cache().map_err(|e| e.to_string())?;
    let mut logits = vec![0.0f32; model.config().vocab];
    let times = sample(|| {
        kv.clear();
        for i in 0..TOKENS {
            model
                .forward(&mut kv, (i * 31) as u32, Some(&mut logits))
                .expect("a token of the generated model");
        }
    });
    let rate = TOKENS as f64 / times[0].as_secs_f64();
    println!(
        "\nCheck: the engine's whole forward pass on a generated model with Qwen3-0.6B's \
         shapes, one thread, {}: {rate:.1} tokens/s. The one-thread projection for \
         qwen3-0.6b above leaves out norms, RoPE and attention, so it should come out \
         at or above this; one-thread figures on a desktop vary by up to a fifth over \
         a run (clocks, other programs).",
        tier.name()
    );
    Ok(())
}

/// The CPU's brand string, from CPUID leaves 0x8000_0002 to 0x8000_0004.
fn cpu_name() -> String {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::__cpuid;
        // SAFETY: CPUID exists on every x86_64 CPU and only reads.
        #[allow(unused_unsafe)]
        let top = unsafe { __cpuid(0x8000_0000) }.eax;
        if top >= 0x8000_0004 {
            let mut bytes = Vec::new();
            for leaf in 0x8000_0002..=0x8000_0004u32 {
                // SAFETY: as above; the leaf is at most the highest one reported.
                #[allow(unused_unsafe)]
                let r = unsafe { __cpuid(leaf) };
                for reg in [r.eax, r.ebx, r.ecx, r.edx] {
                    bytes.extend_from_slice(&reg.to_le_bytes());
                }
            }
            return String::from_utf8_lossy(&bytes)
                .trim_matches(['\0', ' '])
                .to_string();
        }
    }
    "an unnamed CPU".into()
}
