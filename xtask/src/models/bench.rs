//! `cargo xtask bench quant`: how fast this PC runs the engine, measured rather
//! than estimated. It times the Q8_0 matvec at Qwen3-0.6B's shapes in every tier,
//! then whole tokens of a generated model with Qwen3-0.6B's shapes and random
//! weights, so no download is needed.

use quant::{QVec, Tier, matvec, row_bytes};
use std::time::{Duration, Instant};

/// Qwen3-0.6B's matrices: (inputs, outputs).
const SHAPES: [(&str, usize, usize); 3] = [
    ("1024 -> 3072 (ffn_up)", 1024, 3072),
    ("3072 -> 1024 (ffn_down)", 3072, 1024),
    ("1024 -> 151936 (output)", 1024, 151_936),
];
const TOKENS: usize = 12;

pub fn run() -> Result<(), String> {
    let cpu = cpufeat::Features::detect();
    let sets: Vec<&str> = cpu.usable().collect();
    println!("xtask: CPU vector sets: {}", sets.join(" "));
    println!("xtask: Q8_0 matvec, one thread");
    for (name, cols, rows) in SHAPES {
        let w = weights(cols, rows);
        let mut x = QVec::new(cols);
        x.quantize(&(0..cols).map(|i| (i % 7) as f32 - 3.0).collect::<Vec<_>>());
        let mut y = vec![0.0f32; rows];
        for tier in Tier::all() {
            let per_call = time(|| matvec(tier, &w, &x, &mut y));
            let gbps = w.len() as f64 / per_call.as_secs_f64() / 1e9;
            println!(
                "  {name:<26} {:<9} {:>8.3} ms  {gbps:>5.1} GB/s",
                tier.name(),
                per_call.as_secs_f64() * 1e3
            );
        }
    }
    println!("xtask: generating a model with Qwen3-0.6B's shapes (random weights)");
    let bytes = infer::fixture::build(&infer::fixture::Spec::qwen3_0_6b());
    let file = gguf::Gguf::parse(&bytes).map_err(|e| e.to_string())?;
    for tier in Tier::all() {
        let mut model = infer::Model::new(&file, tier, 64).map_err(|e| e.to_string())?;
        let mut kv = model.kv_cache().map_err(|e| e.to_string())?;
        let mut logits = vec![0.0f32; model.config().vocab];
        let start = Instant::now();
        for i in 0..TOKENS {
            model
                .forward(&mut kv, (i * 31) as u32, Some(&mut logits))
                .map_err(|e| e.to_string())?;
        }
        let rate = TOKENS as f64 / start.elapsed().as_secs_f64();
        println!(
            "  Qwen3-0.6B shapes        {:<9} {rate:>6.1} tokens/s",
            tier.name()
        );
    }
    println!(
        "xtask: this is cargo's dev build with the engine at opt-level 2; a release \
         build runs about 10-20% faster. The VM adds its own overhead."
    );
    Ok(())
}

fn weights(cols: usize, rows: usize) -> Vec<u8> {
    let mut w = vec![0u8; rows * row_bytes(cols)];
    let mut state = 0x2545_f491_4f6c_dd1du64;
    for block in w.chunks_mut(quant::BLOCK_BYTES) {
        block[..2].copy_from_slice(&quant::f16::from_f32(0.01).to_le_bytes());
        for b in &mut block[2..] {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *b = state as u8;
        }
    }
    w
}

/// The time one call takes, from as many calls as fit in about half a second.
fn time(mut f: impl FnMut()) -> Duration {
    f();
    let start = Instant::now();
    let mut calls = 0u32;
    while start.elapsed() < Duration::from_millis(500) {
        f();
        calls += 1;
    }
    start.elapsed() / calls
}
