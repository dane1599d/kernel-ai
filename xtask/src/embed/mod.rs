//! `cargo xtask embed`: spike S13-3 `embed` (ADR 0013), Qwen3-Embedding-0.6B
//! Q8_0 through `infer` with last-token pooling. It runs a real model, so it
//! runs on GitHub's runner (s13-3-embed.yml), never on the founder's PC.
//!
//! - `embed strings --out FILE`: the 100 texts llama.cpp embeds too: the 50
//!   queries as `search` embeds them (with the query instruction) and every
//!   fourth note.
//! - `embed check --file GGUF --llama FILE`: our token ids and vectors for
//!   them against llama.cpp's `--pooling last` (bar: cosine at least 0.999).
//! - `embed bench --file GGUF`: recall@5 on the corpus (bar 0.8) at 256
//!   dimensions stored as f16, as `search` keeps them, and at 1,024; indexing
//!   tokens/s on 400-token chunks; and a query's latency over 10,000 chunks
//!   (bar 300 ms). The guest's tokens/s under KVM is the bar for indexing
//!   (150); this is the host's figure beside it.

pub mod corpus;

use crate::models::hostpool::HostPool;
use crate::models::{Loaded, ModelArgs};
use corpus::Corpus;
use quant::{Parallel, Serial, Tier};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Qwen3-Embedding's query form; notes are embedded as they are.
pub const QUERY_INSTRUCTION: &str =
    "Instruct: Given a search query, retrieve the notes and files that answer it\nQuery:";
/// `search`'s vectors: the first 256 of the model's 1,024 (MRL), renormalised.
const DIMS: usize = 256;
const CHUNKS: usize = 10_000;
const CHUNK_TOKENS: usize = 400;
const CONTEXT: usize = 512;
const TOP: usize = 5;
const COSINE_BAR: f32 = 0.999;

pub fn main(args: &[String]) -> Result<(), String> {
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from)
    };
    match args.first().map(String::as_str) {
        Some("strings") => {
            let out = flag("--out").ok_or("usage: cargo xtask embed strings --out FILE")?;
            let text = serde_json::to_string_pretty(&strings(&corpus::build()))
                .map_err(|e| e.to_string())?;
            fs::write(&out, text).map_err(|e| format!("cannot write {}: {e}", out.display()))
        }
        Some("check") => {
            let (Some(file), Some(llama)) = (flag("--file"), flag("--llama")) else {
                return Err("usage: cargo xtask embed check --file GGUF --llama FILE".into());
            };
            check(file, llama)
        }
        Some("bench") => {
            let file = flag("--file").ok_or("usage: cargo xtask embed bench --file GGUF")?;
            bench(file)
        }
        _ => Err("usage: cargo xtask embed strings|check|bench ...".into()),
    }
}

/// The texts compared with llama.cpp.
fn strings(corpus: &Corpus) -> Vec<String> {
    let queries = corpus
        .queries
        .iter()
        .map(|q| format!("{QUERY_INSTRUCTION}{}", q.text));
    let notes = corpus.docs.iter().step_by(4).map(|d| d.text.clone());
    queries.chain(notes).collect()
}

/// The model with a KV cache for one text at a time, and its tokenizer.
struct Embedder<'a> {
    model: infer::Model<'a>,
    kv: infer::KvCache,
    tok: bpe::Tokenizer,
    eos: u32,
    width: usize,
}

impl<'a> Embedder<'a> {
    fn new(file: &gguf::Gguf<'a>, pool: &'a dyn Parallel) -> Result<Self, String> {
        let mut model =
            infer::Model::new(file, Tier::best(), CONTEXT).map_err(|e| e.to_string())?;
        if model.config().pooling != infer::Pooling::Last {
            return Err("the file is not a last-token-pooling embedding model".into());
        }
        model.set_parallel(pool);
        let kv = model.kv_cache().map_err(|e| e.to_string())?;
        let tok = bpe::Tokenizer::from_gguf(file.header()).map_err(|e| e.to_string())?;
        let eos = tok
            .special("<|endoftext|>")
            .ok_or("the vocabulary has no <|endoftext|>")?;
        let width = model.config().n_embd;
        Ok(Self {
            model,
            kv,
            tok,
            eos,
            width,
        })
    }

    /// A text's ids as llama.cpp tokenizes it for an embedding: the end-of-text
    /// token appended (`tokenizer.ggml.add_eos_token`).
    fn tokens(&self, text: &str) -> Vec<u32> {
        let mut ids = Vec::new();
        self.tok.encode(text, &mut ids);
        ids.push(self.eos);
        ids
    }

    fn embed_ids(&mut self, ids: &[u32]) -> Result<Vec<f32>, String> {
        self.kv.clear();
        let mut out = vec![0.0f32; self.width];
        self.model
            .embed(&mut self.kv, ids, &mut out)
            .map_err(|e| e.to_string())?;
        Ok(out)
    }

    fn embed(&mut self, text: &str) -> Result<Vec<f32>, String> {
        let ids = self.tokens(text);
        self.embed_ids(&ids)
    }
}

fn pool() -> Box<dyn Parallel> {
    match std::thread::available_parallelism().map_or(1, |n| n.get()) {
        1 => Box::new(Serial),
        n => Box::new(HostPool::new(n)),
    }
}

fn open(file: PathBuf) -> Result<Loaded, String> {
    Loaded::open(&ModelArgs {
        file: Some(file),
        ..ModelArgs::default()
    })
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

fn norm(a: &[f32]) -> f32 {
    dot(a, a).sqrt()
}

fn check(file: PathBuf, llama: PathBuf) -> Result<(), String> {
    let text = fs::read_to_string(&llama).map_err(|e| format!("{}: {e}", llama.display()))?;
    let theirs: Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", llama.display()))?;
    let texts = strings(&corpus::build());
    let loaded = open(file)?;
    let gguf = loaded.gguf()?;
    let pool = pool();
    let mut embedder = Embedder::new(&gguf, pool.as_ref())?;
    let (mut worst, mut sum, mut token_diffs) = (f32::MAX, 0.0f32, 0);
    let mut scores = Vec::new();
    for (i, text) in texts.iter().enumerate() {
        let ids = embedder.tokens(text);
        let their_ids: Vec<u32> = theirs["tokens"][i]
            .as_array()
            .ok_or(format!("no llama.cpp tokens for string {i}"))?
            .iter()
            .filter_map(|v| v.as_u64().map(|v| v as u32))
            .collect();
        if ids != their_ids {
            token_diffs += 1;
            println!("xtask: string {i}: our ids {ids:?}, llama.cpp's {their_ids:?}");
        }
        let vector: Vec<f32> = theirs["embeddings"][i]
            .as_array()
            .ok_or(format!("no llama.cpp embedding for string {i}"))?
            .iter()
            .filter_map(|v| v.as_f64().map(|v| v as f32))
            .collect();
        let ours = embedder.embed_ids(&ids)?;
        if vector.len() != ours.len() {
            return Err(format!(
                "string {i}: llama.cpp gave {} values",
                vector.len()
            ));
        }
        let cosine = dot(&ours, &vector) / norm(&vector);
        worst = worst.min(cosine);
        sum += cosine;
        scores.push((cosine, i));
    }
    scores.sort_by(|a, b| a.0.total_cmp(&b.0));
    for &(cosine, i) in scores.iter().take(3) {
        println!(
            "xtask: lowest: string {i} cosine {cosine:.6}: {:?}",
            texts[i]
        );
    }
    let n = texts.len();
    let verdict = if worst >= COSINE_BAR && token_diffs == 0 {
        "PASS"
    } else {
        "FAIL"
    };
    println!(
        "xtask: S13-3 cosine against llama.cpp --pooling last over {n} strings: min {worst:.6}, mean {:.6}; token ids differ on {token_diffs} (bar: min at least {COSINE_BAR}, ids equal): {verdict}",
        sum / n as f32
    );
    match verdict {
        "PASS" => Ok(()),
        _ => Err("S13-3: the embeddings do not match llama.cpp's".into()),
    }
}

/// The first `DIMS` values, renormalised, through f16 and back, as `search`
/// stores them.
fn stored(full: &[f32]) -> Vec<u16> {
    let cut = &full[..DIMS];
    let n = norm(cut);
    cut.iter().map(|v| quant::f16::from_f32(v / n)).collect()
}

fn widened(stored: &[u16]) -> Vec<f32> {
    stored.iter().map(|&h| quant::f16::to_f32(h)).collect()
}

/// The `TOP` best of `scores` (index, score), best first.
fn top(mut scores: Vec<(usize, f32)>) -> Vec<usize> {
    let k = TOP.min(scores.len());
    if k == 0 {
        return Vec::new();
    }
    scores.select_nth_unstable_by(k - 1, |a, b| b.1.total_cmp(&a.1));
    scores.truncate(k);
    scores.sort_by(|a, b| b.1.total_cmp(&a.1));
    scores.into_iter().map(|(i, _)| i).collect()
}

fn in_window(window: Option<(u32, u32)>, day: u32) -> bool {
    window.is_none_or(|(since, until)| (since..=until).contains(&day))
}

fn bench(file: PathBuf) -> Result<(), String> {
    let corpus = corpus::build();
    let loaded = open(file)?;
    let gguf = loaded.gguf()?;
    let pool = pool();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut embedder = Embedder::new(&gguf, pool.as_ref())?;
    println!(
        "xtask: S13-3 on the host, {threads} threads, tier {}",
        Tier::best().name()
    );

    let started = Instant::now();
    let mut doc_tokens = 0;
    let mut docs_full = Vec::new();
    for doc in &corpus.docs {
        let ids = embedder.tokens(&doc.text);
        doc_tokens += ids.len();
        docs_full.push(embedder.embed_ids(&ids)?);
    }
    let docs_time = started.elapsed();
    let queries_full: Vec<Vec<f32>> = corpus
        .queries
        .iter()
        .map(|q| embedder.embed(&format!("{QUERY_INSTRUCTION}{}", q.text)))
        .collect::<Result<_, _>>()?;
    let docs_stored: Vec<Vec<f32>> = docs_full.iter().map(|d| widened(&stored(d))).collect();

    let recall = |docs: &[Vec<f32>], cut: usize, timed_only: bool| -> (usize, usize) {
        let mut hits = 0;
        let mut asked = 0;
        for (q, query) in corpus.queries.iter().zip(&queries_full) {
            if timed_only && q.window.is_none() {
                continue;
            }
            asked += 1;
            let qv = &query[..cut];
            let qn = norm(qv);
            let scores = docs
                .iter()
                .enumerate()
                .filter(|(i, _)| in_window(q.window, corpus.docs[*i].day))
                .map(|(i, d)| (i, dot(&d[..cut.min(d.len())], qv) / qn))
                .collect();
            if top(scores).contains(&q.answer) {
                hits += 1;
            } else if !timed_only && cut == DIMS {
                println!("xtask: missed at {DIMS} dims: {:?}", q.text);
            }
        }
        (hits, asked)
    };
    let (h256, n) = recall(&docs_stored, DIMS, false);
    let (h1024, _) = recall(&docs_full, 1024, false);
    let (timed256, timed_n) = recall(&docs_stored, DIMS, true);
    let r256 = h256 as f32 / n as f32;
    println!(
        "xtask: S13-3 recall@5 over {} notes and {n} queries: {r256:.2} at {DIMS} dims in f16 ({h256}/{n}), {:.2} at 1,024 ({h1024}/{n}); time-filtered {timed256}/{timed_n} (bar 0.8): {}",
        corpus.docs.len(),
        h1024 as f32 / n as f32,
        if r256 >= 0.8 { "PASS" } else { "FAIL" }
    );
    println!(
        "xtask: the 200 notes ({doc_tokens} tokens) embedded in {} ms on the host: {:.0} tokens/s",
        docs_time.as_millis(),
        doc_tokens as f64 / docs_time.as_secs_f64()
    );

    // Indexing: 400-token chunks of the notes run together, as `search` cuts files.
    let mut all = Vec::new();
    for doc in corpus.docs.iter().cycle().take(600) {
        embedder.tok.encode(&doc.text, &mut all);
        embedder.tok.encode("\n\n", &mut all);
    }
    let chunks: Vec<Vec<u32>> = all
        .chunks(CHUNK_TOKENS - 1)
        .filter(|c| c.len() == CHUNK_TOKENS - 1)
        .take(12)
        .map(|c| {
            let mut c = c.to_vec();
            c.push(embedder.eos);
            c
        })
        .collect();
    let started = Instant::now();
    for chunk in &chunks {
        embedder.embed_ids(chunk)?;
    }
    let chunk_time = started.elapsed();
    let chunk_tokens = chunks.len() * CHUNK_TOKENS;
    println!(
        "xtask: S13-3 indexing on the host: {} chunks of {CHUNK_TOKENS} tokens in {} ms, {:.0} tokens/s (the bar, 150, is the guest's under KVM)",
        chunks.len(),
        chunk_time.as_millis(),
        chunk_tokens as f64 / chunk_time.as_secs_f64()
    );

    latency(&mut embedder, &corpus)
}

/// A query against 10,000 stored chunks: embed it, scan them all with the time
/// filter, keep the best five. The chunks' vectors are random unit vectors:
/// the scan's cost does not depend on what they say.
fn latency(embedder: &mut Embedder<'_>, corpus: &Corpus) -> Result<(), String> {
    let mut seed = 0x5133_0003_u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f32 / (1u64 << 53) as f32 - 0.5
    };
    let chunks: Vec<(Vec<u16>, u32)> = (0..CHUNKS)
        .map(|i| {
            let v: Vec<f32> = (0..DIMS).map(|_| next()).collect();
            (stored(&v), (i % 365) as u32)
        })
        .collect();
    let mut embed_times = Vec::new();
    let mut scan_times = Vec::new();
    let mut totals = Vec::new();
    for q in corpus.queries.iter().take(20) {
        let start = Instant::now();
        let full = embedder.embed(&format!("{QUERY_INSTRUCTION}{}", q.text))?;
        let embedded = start.elapsed();
        let qv = widened(&stored(&full));
        let scores: Vec<(usize, f32)> = chunks
            .iter()
            .enumerate()
            .filter(|(_, (_, day))| in_window(q.window, *day))
            .map(|(i, (v, _))| {
                let s: f32 = v
                    .iter()
                    .zip(&qv)
                    .map(|(&h, q)| quant::f16::to_f32(h) * q)
                    .sum();
                (i, s)
            })
            .collect();
        std::hint::black_box(top(scores));
        let total = start.elapsed();
        embed_times.push(embedded);
        scan_times.push(total - embedded);
        totals.push(total);
    }
    let median = |mut v: Vec<Duration>| {
        v.sort();
        v[v.len() / 2]
    };
    let worst = *totals.iter().max().unwrap_or(&Duration::ZERO);
    let p50 = median(totals);
    println!(
        "xtask: S13-3 query over {CHUNKS} chunks on the host: p50 {} ms (embed {} ms, scan and top-5 {} ms), max {} ms (bar 300 ms in the guest)",
        p50.as_millis(),
        median(embed_times).as_millis(),
        median(scan_times).as_millis(),
        worst.as_millis()
    );
    let summary = json!({ "p50_ms": p50.as_millis() as u64, "max_ms": worst.as_millis() as u64 });
    println!("xtask: latency {summary}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use infer::fixture::{self, Spec};

    #[test]
    fn the_embedder_appends_end_of_text_and_gives_unit_vectors() {
        let bytes = fixture::build(&Spec {
            pooling_type: Some(3),
            ..Spec::tiny()
        });
        let file = gguf::Gguf::parse(&bytes).unwrap();
        let mut embedder = Embedder::new(&file, &Serial).unwrap();
        let ids = embedder.tokens("budget for the Leeds office");
        assert_eq!(ids.last(), Some(&embedder.eos));
        let a = embedder.embed("budget for the Leeds office").unwrap();
        let again = embedder.embed("budget for the Leeds office").unwrap();
        assert_eq!(a, again, "the cache is cleared between texts");
        assert!((norm(&a) - 1.0).abs() < 1e-5);
        // A chat model is refused.
        let chat = fixture::build(&Spec::tiny());
        let chat = gguf::Gguf::parse(&chat).unwrap();
        assert!(Embedder::new(&chat, &Serial).is_err());
    }

    #[test]
    fn storing_cuts_to_256_unit_dims_and_top_ranks_best_first() {
        let full: Vec<f32> = (0..1024).map(|i| (i % 7) as f32 - 3.0).collect();
        let kept = widened(&stored(&full));
        assert_eq!(kept.len(), DIMS);
        assert!((norm(&kept) - 1.0).abs() < 1e-3);
        let scores = vec![(0, 0.1), (1, 0.9), (2, 0.5), (3, 0.7), (4, 0.2), (5, 0.8)];
        assert_eq!(top(scores), vec![1, 5, 3, 2, 4]);
        assert!(in_window(None, 7) && in_window(Some((5, 9)), 9) && !in_window(Some((5, 9)), 10));
    }

    #[test]
    fn the_hundred_strings_are_fifty_queries_and_fifty_notes() {
        let texts = strings(&corpus::build());
        assert_eq!(texts.len(), 100);
        assert!(texts[..50].iter().all(|t| t.starts_with(QUERY_INSTRUCTION)));
        assert!(texts[50..].iter().all(|t| !t.starts_with("Instruct:")));
    }
}
