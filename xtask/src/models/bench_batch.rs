//! `cargo xtask bench batch`: ADR 0013 spike S13-2 `batch-decode` on the host.
//! One decode step of 1, 2 and 4 sequences together ([`infer::Model::decode_batch`],
//! each matrix row read once for all of them through `quant`'s 8-token kernels)
//! on a generated model with Qwen3-4B's layer shapes in its Q4_K_M mix, as
//! `bench quant`'s last check builds it. Two models of [`LAYERS`] and
//! [`FEWER`] layers give one layer's time and the rest (embeddings and the
//! output projection, which a batch computes for every sequence), so the 4B's
//! step is 36 layers' time plus the rest. Each sequence decodes after
//! [`SHORT`] and again after [`LONG`] cached positions of its own. The pass is
//! S13-2's: at least 1.6x the one sequence's total tokens/s at 2 sequences.
//! Weights are random, so only the speed is the real model's.

use super::bench::sample;
use super::hostpool::HostPool;
use quant::{Parallel, Serial, Tier};

const LAYERS: usize = 8;
const FEWER: usize = 4;
const REAL_LAYERS: usize = 36;
/// Decode steps timed per run.
const STEPS: usize = 8;
/// Cached positions before decoding: a short request, and a chat's system
/// prompt and a few turns.
const SHORT: usize = 32;
const LONG: usize = 1024;
/// Sequences decoded together.
const BATCHES: [usize; 4] = [1, 2, 4, 8];
/// S13-2's bar at 2 sequences.
const PASS: f64 = 1.6;

/// Seconds for [`STEPS`] decode steps of `n` sequences, each after `cached`
/// positions, on `file`.
fn steps(
    file: &gguf::Gguf<'_>,
    pool: &dyn Parallel,
    n: usize,
    cached: usize,
) -> Result<f64, String> {
    let tier = Tier::best();
    let mut model = infer::Model::new(file, tier, cached + STEPS + 1).map_err(|e| e.to_string())?;
    model.set_parallel(pool);
    let vocab = model.config().vocab;
    let mut kvs = Vec::new();
    for s in 0..n {
        let mut kv = model.kv_cache().map_err(|e| e.to_string())?;
        let prompt: Vec<u32> = (0..cached as u32)
            .map(|i| (i * 31 + 7 + s as u32) % 1000)
            .collect();
        model
            .forward_batch(&mut kv, &prompt, None)
            .map_err(|e| e.to_string())?;
        kvs.push(kv);
    }
    let mut logits = vec![0.0f32; n * vocab];
    let tokens: Vec<u32> = (0..n as u32).map(|s| 97 + s).collect();
    let time = sample(|| {
        for kv in &mut kvs {
            kv.truncate(cached);
        }
        for _ in 0..STEPS {
            let mut refs: Vec<&mut infer::KvCache> = kvs.iter_mut().collect();
            match n {
                1 => model.forward(refs[0], tokens[0], Some(&mut logits)),
                _ => model.decode_batch(&mut refs, &tokens, &mut logits),
            }
            .expect("a decode step");
        }
    })[0];
    Ok(time.as_secs_f64())
}

/// One 4B decode step of `n` sequences after `cached` positions, in seconds.
fn step(
    fewer: &gguf::Gguf<'_>,
    more: &gguf::Gguf<'_>,
    pool: &dyn Parallel,
    n: usize,
    cached: usize,
) -> Result<f64, String> {
    let (few, many) = (
        steps(fewer, pool, n, cached)?,
        steps(more, pool, n, cached)?,
    );
    let per_layer = ((many - few) / (LAYERS - FEWER) as f64).max(0.0);
    let rest = (few - per_layer * FEWER as f64).max(0.0);
    Ok((per_layer * REAL_LAYERS as f64 + rest) / STEPS as f64)
}

/// Tokens a second through one prompt block of `infer::PREFILL_TOKENS`, scaled
/// to the 4B's layers without the output projection: what the products run at
/// when 32 tokens share each row read, the most a batch's layers could reach.
fn prefill(
    fewer: &gguf::Gguf<'_>,
    more: &gguf::Gguf<'_>,
    pool: &dyn Parallel,
) -> Result<f64, String> {
    let block = |file: &gguf::Gguf<'_>| -> Result<f64, String> {
        let mut model = infer::Model::new(file, Tier::best(), SHORT + infer::PREFILL_TOKENS)
            .map_err(|e| e.to_string())?;
        model.set_parallel(pool);
        let mut kv = model.kv_cache().map_err(|e| e.to_string())?;
        let prompt: Vec<u32> = (0..infer::PREFILL_TOKENS as u32)
            .map(|i| i * 31 + 7)
            .collect();
        Ok(sample(|| {
            kv.clear();
            model
                .forward_batch(&mut kv, &prompt, None)
                .expect("a block");
        })[0]
            .as_secs_f64())
    };
    let per_layer = ((block(more)? - block(fewer)?) / (LAYERS - FEWER) as f64).max(1e-9);
    Ok(infer::PREFILL_TOKENS as f64 / (per_layer * REAL_LAYERS as f64))
}

pub fn run() -> Result<(), String> {
    let build = |layers| infer::fixture::build(&infer::fixture::Spec::qwen3_4b_layers(layers));
    let (fewer, more) = (build(FEWER), build(LAYERS));
    let fewer = gguf::Gguf::parse(&fewer).map_err(|e| e.to_string())?;
    let more = gguf::Gguf::parse(&more).map_err(|e| e.to_string())?;
    let logical = std::thread::available_parallelism().map_or(1, |n| n.get());
    let threads = [1, logical.clamp(1, 8)];
    println!(
        "xtask: bench batch: S13-2 batch-decode, {} tier, Qwen3-4B shapes (Q4_K_M mix, random weights), {LAYERS} and {FEWER} layers scaled to {REAL_LAYERS}, {STEPS} steps a run, median of the runs",
        Tier::best().name()
    );
    let mut passes = true;
    for n_threads in threads {
        let pool: Box<dyn Parallel> = match n_threads {
            1 => Box::new(Serial),
            n => Box::new(HostPool::new(n)),
        };
        for cached in [SHORT, LONG] {
            let mut one = 0.0;
            let mut cells = Vec::new();
            for n in BATCHES {
                let secs = step(&fewer, &more, pool.as_ref(), n, cached)?;
                let total = n as f64 / secs;
                if n == 1 {
                    one = total;
                }
                let gain = total / one;
                if n == 2 && n_threads > 1 {
                    passes &= gain >= PASS;
                }
                cells.push(format!(
                    "{n} seq: {total:.1} tokens/s total, {:.1} each ({gain:.2}x)",
                    total / n as f64
                ));
            }
            if cached == SHORT {
                cells.push(format!(
                    "prefill block, layers only: {:.1} tokens/s",
                    prefill(&fewer, &more, pool.as_ref())?
                ));
            }
            println!(
                "xtask: bench batch: {n_threads} thread{}, after {cached} positions: {}",
                if n_threads == 1 { "" } else { "s" },
                cells.join("; ")
            );
        }
    }
    println!(
        "xtask: bench batch: S13-2 {} (at least {PASS}x total at 2 sequences on the pool)",
        if passes { "passes" } else { "fails" }
    );
    Ok(())
}
