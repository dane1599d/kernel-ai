//! Spike S13-3 `embed`'s guest figures (ADR 0013), not for merge: the embedding
//! model on the `kai-model` disk (Qwen3-Embedding-0.6B Q8_0 on GitHub's runner)
//! through `infer`'s last-token pooling, in hard-float ring 3 on every CPU's
//! worker, as `search`'s `index` agent will run it. It prints indexing tokens/s
//! on 400-token chunks (bar 150 under KVM) and a query's latency over 10,000
//! stored chunks of 256 f16 dimensions with a time filter (bar 300 ms). The
//! figures are reported, not asserted; it exits 0 when the model loads and every
//! vector has unit length. Boot case `test=embed-bench`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use quant::{Parallel, Serial, Tier};
use user::pool::Pool;
use user::{println, sys};

user::entry!(main);

const CONTEXT: usize = 512;
/// Prompt blocks of 128 tokens, against `infer::PREFILL_TOKENS` (32) for chat:
/// the cheap speed lever the manager asked to measure.
const BLOCK: usize = 128;
const CHUNK_TOKENS: usize = 400;
const CHUNKS_TIMED: usize = 6;
const STORED: usize = 10_000;
const DIMS: usize = 256;
const QUERIES: usize = 9;
const QUERY: &str = "Instruct: Given a search query, retrieve the notes and files that answer it\nQuery:the electrician's bill for the weekend emergency visit";
/// Ordinary prose for the chunks: tokens/s does not depend on what they say.
const PROSE: &str = "Meeting minutes: the finance team agreed to cut the travel budget by the end of the quarter, and support plans to recruit two more people. ";

struct Workers(Pool);

impl Parallel for Workers {
    fn workers(&self) -> usize {
        self.0.workers()
    }

    fn broadcast(&self, job: &(dyn Fn(usize, usize) + Sync)) {
        self.0.broadcast(job);
    }
}

fn workers() -> &'static dyn Parallel {
    match Pool::new(kabi::thread::MAX_WORKERS) {
        Ok(pool) => {
            let workers: &'static Workers = Box::leak(Box::new(Workers(pool)));
            if workers.0.workers() >= 3 {
                let _ = sys::thread_pin(Some(0));
            }
            workers
        }
        Err(_) => &Serial,
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

/// The first 256 values, renormalised, in f16, as `search` stores them.
fn stored(full: &[f32]) -> Vec<u16> {
    let cut = &full[..DIMS];
    let n = libm::sqrtf(dot(cut, cut));
    cut.iter().map(|v| quant::f16::from_f32(v / n)).collect()
}

fn median(mut v: Vec<u64>) -> u64 {
    v.sort_unstable();
    v[v.len() / 2]
}

fn main() -> i32 {
    let len = match sys::model_map() {
        Ok(len) => len as usize,
        Err(error) => {
            println!("embed-bench: no model to map: {error:?}");
            return 1;
        }
    };
    // SAFETY: MODEL_MAP mapped `len` readable bytes at MODEL_BASE for the rest
    // of the process's life.
    let bytes = unsafe { core::slice::from_raw_parts(kabi::layout::MODEL_BASE as *const u8, len) };
    let Ok(file) = gguf::Gguf::parse(bytes) else {
        println!("embed-bench: the disk is not a GGUF file");
        return 1;
    };
    let tier = Tier::best();
    let mut model = match infer::Model::with_block(&file, tier, CONTEXT, BLOCK) {
        Ok(model) => model,
        Err(error) => {
            println!("embed-bench: the engine refuses the model: {error}");
            return 1;
        }
    };
    if model.config().pooling != infer::Pooling::Last {
        println!("embed-bench: not a last-token-pooling embedding model");
        return 1;
    }
    let par = workers();
    model.set_parallel(par);
    let Ok(tok) = bpe::Tokenizer::from_gguf(file.header()) else {
        println!("embed-bench: no tokenizer in the file");
        return 1;
    };
    let Some(eos) = tok.special("<|endoftext|>") else {
        println!("embed-bench: no <|endoftext|> in the vocabulary");
        return 1;
    };
    let Ok(mut kv) = model.kv_cache() else {
        println!("embed-bench: no room for the KV cache");
        return 1;
    };
    let width = model.config().n_embd;
    println!(
        "embed-bench: {} MB model, {} workers, tier {}, width {width}",
        len / 1_000_000,
        par.workers(),
        tier.name()
    );
    let mut right = true;
    let mut embed = |ids: &[u32], out: &mut Vec<f32>| -> bool {
        kv.clear();
        out.resize(width, 0.0);
        model.embed(&mut kv, ids, out).is_ok() && (dot(out, out) - 1.0).abs() < 1e-3
    };

    // Indexing: chunks of 399 tokens of prose and the end-of-text token.
    let mut prose = Vec::new();
    while prose.len() < CHUNK_TOKENS * CHUNKS_TIMED {
        tok.encode(PROSE, &mut prose);
    }
    let mut vector = Vec::new();
    // A warm-up chunk first: the first touch of each weight page is not indexing.
    let mut chunk: Vec<u32> = prose[..CHUNK_TOKENS - 1].to_vec();
    chunk.push(eos);
    right &= embed(&chunk, &mut vector);
    // ADR 0013's 400-token chunks (the bar), then shorter ones: attention's
    // cost grows with a chunk's length, so the chunk size is a lever too.
    for size in [CHUNK_TOKENS, 128] {
        let count = CHUNKS_TIMED * CHUNK_TOKENS / size;
        let started = sys::time_ns();
        for c in 0..count {
            let mut chunk: Vec<u32> = prose[c * (size - 1)..][..size - 1].to_vec();
            chunk.push(eos);
            right &= embed(&chunk, &mut vector);
        }
        let ns = sys::time_ns() - started;
        let tokens = (count * size) as u64;
        println!(
            "embed-bench: indexing {count} chunks of {size} tokens in {} ms: {} tokens/s, prefill blocks of {} (bar 150 under KVM, at {CHUNK_TOKENS})",
            ns / 1_000_000,
            tokens * 1_000_000_000 / ns.max(1),
            BLOCK
        );
    }

    // A query against 10,000 stored chunks of random unit vectors, half of them
    // inside the time window: embed, filtered scan, the best five.
    let mut seed = 0x5133_0003_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f32 / (1u64 << 53) as f32 - 0.5
    };
    let store: Vec<(Vec<u16>, u32)> = (0..STORED)
        .map(|i| {
            let v: Vec<f32> = (0..DIMS).map(|_| next()).collect();
            (stored(&v), (i % 365) as u32)
        })
        .collect();
    let mut ids = Vec::new();
    tok.encode(QUERY, &mut ids);
    ids.push(eos);
    let (mut totals, mut embeds) = (Vec::new(), Vec::new());
    let mut q = vec![0.0f32; DIMS];
    for _ in 0..QUERIES {
        let start = sys::time_ns();
        right &= embed(&ids, &mut vector);
        let embedded = sys::time_ns();
        for (q, h) in q.iter_mut().zip(stored(&vector)) {
            *q = quant::f16::to_f32(h);
        }
        let mut scores: Vec<(usize, f32)> = store
            .iter()
            .enumerate()
            .filter(|(_, (_, day))| (0..=182).contains(day))
            .map(|(i, (v, _))| {
                let s: f32 = v
                    .iter()
                    .zip(&q)
                    .map(|(&h, q)| quant::f16::to_f32(h) * q)
                    .sum();
                (i, s)
            })
            .collect();
        scores.select_nth_unstable_by(4, |a, b| b.1.total_cmp(&a.1));
        core::hint::black_box(&scores[..5]);
        let done = sys::time_ns();
        embeds.push(embedded - start);
        totals.push(done - start);
    }
    let worst = totals.iter().copied().max().unwrap_or(0);
    let query_tokens = ids.len();
    println!(
        "embed-bench: a query ({query_tokens} tokens) over {STORED} chunks: p50 {} ms (embed {} ms), max {} ms (bar 300 ms)",
        median(totals) / 1_000_000,
        median(embeds) / 1_000_000,
        worst / 1_000_000
    );
    println!(
        "embed-bench: {}",
        if right {
            "every vector had unit length"
        } else {
            "a vector was WRONG"
        }
    );
    println!("embed-bench: done");
    if right { 0 } else { 1 }
}
