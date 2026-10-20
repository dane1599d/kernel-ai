//! `bench quant`'s last check: the engine's whole forward pass (norms, RoPE,
//! attention over the f16 cache, every product) on a generated model with
//! Qwen3-4B's layer shapes in its Q4_K_M mix, on each host pool. Building all 36
//! layers would take 2.5 GB more memory than the check needs, so the model has
//! [`LAYERS`]: the layers alone are timed (prompt tokens skip the output
//! projection) and so is the output projection, and the 4B's decode step is
//! 36 layers' time plus the output's. Weights are random, so only the speed is
//! the real model's.

use super::bench::sample;
use quant::{Parallel, Tier};

/// Layers built; each is about 57 MB, far past the L3 cache, as the real
/// model's are.
const LAYERS: usize = 12;
/// The real model's.
const REAL_LAYERS: usize = 36;
/// Decode tokens timed per run, after a short prompt.
const DECODE: usize = 8;
/// Prompt tokens timed per run: one prefill block.
const PREFILL: usize = infer::PREFILL_TOKENS;

pub fn run(threads: &[usize], pools: &[Box<dyn Parallel>], tier: Tier) -> Result<(), String> {
    let bytes = infer::fixture::build(&infer::fixture::Spec::qwen3_4b_layers(LAYERS));
    let file = gguf::Gguf::parse(&bytes).map_err(|e| e.to_string())?;
    let mut cells = Vec::new();
    for (&n, pool) in threads.iter().zip(pools) {
        let mut model = infer::Model::new(&file, tier, 256).map_err(|e| e.to_string())?;
        model.set_parallel(pool.as_ref());
        let mut kv = model.kv_cache().map_err(|e| e.to_string())?;
        let mut logits = vec![0.0f32; model.config().vocab];
        let prompt: Vec<u32> = (0..PREFILL as u32).map(|i| i * 31 + 7).collect();
        // Decode steps after the prompt, without and with the output projection.
        let mut decode = |with_logits: bool| {
            sample(|| {
                kv.clear();
                model
                    .forward_batch(&mut kv, &prompt[..4], None)
                    .expect("a prompt");
                for i in 0..DECODE {
                    let logits = with_logits.then_some(logits.as_mut_slice());
                    model
                        .forward(&mut kv, (i * 97) as u32, logits)
                        .expect("a decode step");
                }
            })[0]
                .as_secs_f64()
        };
        let layers_only = decode(false);
        let with_output = decode(true);
        let prefill = sample(|| {
            kv.clear();
            model
                .forward_batch(&mut kv, &prompt, None)
                .expect("a block");
        })[0]
            .as_secs_f64();
        let per_layer = layers_only / (DECODE * LAYERS) as f64;
        let output = ((with_output - layers_only) / DECODE as f64).max(0.0);
        let step = per_layer * REAL_LAYERS as f64 + output;
        let prompt_rate = PREFILL as f64 / (prefill * (REAL_LAYERS / LAYERS) as f64);
        cells.push(format!(
            "{n} thread{}: {:.1} tokens/s decoding, {:.0} tokens/s prefill",
            if n == 1 { "" } else { "s" },
            1.0 / step,
            prompt_rate
        ));
    }
    println!(
        "\nCheck: the engine's whole forward pass on a generated model with Qwen3-4B's \
         layer shapes (Q4_K_M mix, random weights), {}, scaled from {LAYERS} layers to \
         {REAL_LAYERS}, at short context (median of the runs): {}. Host threads; the \
         guest's pinned workers hand off somewhat more slowly.",
        tier.name(),
        cells.join("; ")
    );
    Ok(())
}
