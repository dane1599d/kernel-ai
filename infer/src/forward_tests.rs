//! The forward pass and greedy generation against the naive reference, on
//! generated models.

extern crate std;

use crate::fixture::{self, Spec};
use crate::reference::Reference;
use crate::*;
use alloc::vec;
use alloc::vec::Vec;
use gguf::Gguf;
use quant::Tier;

/// The ChatML prompt for one question, through the model's own tokenizer.
fn prompt(bytes: &[u8], question: &str) -> Vec<u32> {
    let file = Gguf::parse(bytes).unwrap();
    let tok = bpe::Tokenizer::from_gguf(file.header()).unwrap();
    let chat = bpe::chat::Chat::new(&tok).unwrap();
    chat.prompt(&tok, None, question)
}

/// Largest difference over the largest reference logit.
fn relative_error(engine: &[f32], reference: &[f32]) -> f32 {
    let scale = reference.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let diff = engine
        .iter()
        .zip(reference)
        .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
    diff / scale
}

fn specs() -> [Spec; 4] {
    let tiny = Spec::tiny();
    [
        tiny,
        Spec {
            untied: true,
            seed: 2,
            ..tiny
        },
        // One KV head per query head, and a wider feed-forward block.
        Spec {
            n_head_kv: 4,
            n_ff: 160,
            seed: 3,
            ..tiny
        },
        // One KV head for eight query heads, heads of 20 (not a multiple of the
        // dot product's 8 lanes), and three layers.
        Spec {
            n_layer: 3,
            n_head: 8,
            n_head_kv: 1,
            head_dim: 20,
            seed: 4,
            ..tiny
        },
    ]
}

#[test]
fn logits_match_the_reference_for_the_prompt_and_32_greedy_tokens() {
    for spec in specs() {
        let bytes = fixture::build(&spec);
        let file = Gguf::parse(&bytes).unwrap();
        let mut model = Model::new(&file, Tier::best(), 128).unwrap();
        let mut kv = model.kv_cache().unwrap();
        let mut reference = Reference::new(&bytes);
        let mut logits = vec![0.0f32; spec.vocab];
        let mut want = Vec::new();
        let prompt = prompt(&bytes, "What does a kernel do?");
        assert!(prompt.len() > 10);
        for &token in &prompt {
            model.forward(&mut kv, token, Some(&mut logits)).unwrap();
            want = reference.forward(token);
            let error = relative_error(&logits, &want);
            assert!(
                error < 1e-3,
                "seed {}: error {error} at a prompt token",
                spec.seed
            );
        }
        let mut tokens = Vec::new();
        for step in 0..32 {
            let token = argmax(&logits);
            assert_eq!(
                token,
                argmax(&want),
                "seed {}: token {step} differs",
                spec.seed
            );
            tokens.push(token);
            model.forward(&mut kv, token, Some(&mut logits)).unwrap();
            want = reference.forward(token);
            let error = relative_error(&logits, &want);
            assert!(
                error < 1e-3,
                "seed {}: error {error} at token {step}",
                spec.seed
            );
        }
        // A model that says the same token every time would prove little.
        tokens.sort_unstable();
        tokens.dedup();
        assert!(tokens.len() >= 3, "seed {}: only {tokens:?}", spec.seed);
    }
}

/// The 32 tokens `generate` hands out, on a fresh cache.
fn greedy(bytes: &[u8], tier: Tier, max_new: usize) -> Vec<u32> {
    let file = Gguf::parse(bytes).unwrap();
    let mut model = Model::new(&file, tier, 128).unwrap();
    let mut kv = model.kv_cache().unwrap();
    let mut out = Vec::new();
    let ended = generate(&mut model, &mut kv, &prompt(bytes, "hi"), max_new, |t| {
        out.push(t);
        Next::Continue
    })
    .unwrap();
    assert_eq!(ended, Ended::MaxTokens);
    out
}

#[test]
fn generate_follows_the_reference() {
    let bytes = fixture::build(&Spec::tiny());
    let tokens = greedy(&bytes, Tier::best(), 32);
    let mut reference = Reference::new(&bytes);
    let mut want = Vec::new();
    for &token in &prompt(&bytes, "hi") {
        want = reference.forward(token);
    }
    for &token in &tokens {
        assert_eq!(token, argmax(&want));
        want = reference.forward(token);
    }
}

#[test]
fn every_tier_gives_the_same_logits_bits() {
    let bytes = fixture::build(&Spec::tiny());
    let file = Gguf::parse(&bytes).unwrap();
    let run = |tier: Tier| {
        let mut model = Model::new(&file, tier, 64).unwrap();
        let mut kv = model.kv_cache().unwrap();
        let mut logits = vec![0.0f32; 384];
        let mut bits = Vec::new();
        for &token in &prompt(&bytes, "same bits?") {
            model.forward(&mut kv, token, Some(&mut logits)).unwrap();
            bits.extend(logits.iter().map(|v| v.to_bits()));
        }
        bits
    };
    let baseline = run(Tier::BASELINE);
    for tier in Tier::all() {
        assert!(
            run(tier) == baseline,
            "{} differs from baseline",
            tier.name()
        );
    }
    assert_eq!(
        greedy(&bytes, Tier::BASELINE, 16),
        greedy(&bytes, Tier::best(), 16)
    );
}

#[test]
fn a_cleared_cache_starts_over() {
    let bytes = fixture::build(&Spec::tiny());
    let file = Gguf::parse(&bytes).unwrap();
    let mut model = Model::new(&file, Tier::best(), 64).unwrap();
    let mut kv = model.kv_cache().unwrap();
    let mut first = vec![0.0f32; 384];
    let mut again = vec![0.0f32; 384];
    model.forward(&mut kv, 5, Some(&mut first)).unwrap();
    model.forward(&mut kv, 6, None).unwrap();
    assert_eq!(kv.len(), 2);
    kv.clear();
    model.forward(&mut kv, 5, Some(&mut again)).unwrap();
    assert_eq!(first, again);
}

#[test]
fn generation_stops_where_it_should() {
    let bytes = fixture::build(&Spec::tiny());
    let file = Gguf::parse(&bytes).unwrap();
    let mut model = Model::new(&file, Tier::best(), 16).unwrap();
    let mut kv = model.kv_cache().unwrap();
    let mut seen = 0;
    let ended = generate(&mut model, &mut kv, &[1, 2, 3], 10, |_| {
        seen += 1;
        if seen == 4 {
            Next::Stop
        } else {
            Next::Continue
        }
    });
    assert_eq!((ended, seen), (Ok(Ended::Stopped), 4));
    // The cache holds the prompt and every token fed back: 3 + 3.
    assert_eq!(kv.len(), 6);
    let mut calls = 0;
    let ended = generate(&mut model, &mut kv, &[7], 100, |_| {
        calls += 1;
        Next::Continue
    });
    // 6 + 1 fills 7 of 16; nine more fit, and the tenth token cannot be fed.
    assert_eq!((ended, calls), (Ok(Ended::ContextFull), 10));
    assert_eq!(kv.len(), 16);
    kv.clear();
    let ended = generate(&mut model, &mut kv, &[1], 0, |_| panic!("no token wanted"));
    assert_eq!(ended, Ok(Ended::MaxTokens));
    assert_eq!(
        generate(&mut model, &mut kv, &[], 5, |_| Next::Continue),
        Err(Error::EmptyPrompt)
    );
    // Refused prompts leave the cache as it was.
    kv.clear();
    let long = [1u32; 17];
    let full = generate(&mut model, &mut kv, &long, 5, |_| Next::Continue);
    assert_eq!(full, Err(Error::ContextFull));
    let bad = generate(&mut model, &mut kv, &[1, 2, 999, 4], 5, |_| Next::Continue);
    assert_eq!(bad, Err(Error::Token(999)));
    assert_eq!(kv.len(), 0);
    assert_eq!(model.forward(&mut kv, 384, None), Err(Error::Token(384)));
    let mut short = [0.0f32; 10];
    assert_eq!(
        model.forward(&mut kv, 1, Some(&mut short)),
        Err(Error::Logits)
    );
    assert_eq!(kv.len(), 0);
    // A prompt that fills the cache exactly gets one token and then stops.
    let exact = [1u32; 16];
    let mut calls = 0;
    let ended = generate(&mut model, &mut kv, &exact, 5, |_| {
        calls += 1;
        Next::Continue
    });
    assert_eq!((ended, calls), (Ok(Ended::ContextFull), 1));
}

#[test]
fn a_cache_for_another_shape_is_refused() {
    let bytes = fixture::build(&Spec::tiny());
    let file = Gguf::parse(&bytes).unwrap();
    let mut model = Model::new(&file, Tier::best(), 64).unwrap();
    let other = fixture::build(&Spec {
        n_head_kv: 4,
        ..Spec::tiny()
    });
    let other_file = Gguf::parse(&other).unwrap();
    let mut kv = Model::new(&other_file, Tier::best(), 64)
        .unwrap()
        .kv_cache()
        .unwrap();
    assert_eq!(model.forward(&mut kv, 1, None), Err(Error::CacheShape));
    // So is a cache longer than the model's context cap.
    let mut long = KvCache::new(model.config(), 128).unwrap();
    assert_eq!(model.forward(&mut long, 1, None), Err(Error::CacheShape));
    let ended = generate(&mut model, &mut long, &[1], 5, |_| Next::Continue);
    assert_eq!(ended, Err(Error::CacheShape));
}

#[test]
fn argmax_picks_as_llama_cpp_does() {
    assert_eq!(argmax(&[1.0, 3.0, 3.0, 2.0]), 1);
    assert_eq!(argmax(&[-1.0, f32::NAN, 2.0]), 2);
    // A NaN at index 0 keeps index 0, as in llama.cpp's greedy sampler.
    assert_eq!(argmax(&[f32::NAN, -1.0, 2.0]), 0);
    assert_eq!(argmax(&[f32::NEG_INFINITY, f32::NEG_INFINITY]), 0);
    assert_eq!(argmax(&[]), 0);
}

#[test]
fn the_context_cap_is_checked() {
    let bytes = fixture::build(&Spec::tiny());
    let file = Gguf::parse(&bytes).unwrap();
    assert!(matches!(
        Model::new(&file, Tier::best(), 0),
        Err(Error::Context)
    ));
    assert!(matches!(
        Model::new(&file, Tier::best(), 513),
        Err(Error::Context)
    ));
    let model = Model::new(&file, Tier::best(), 512).unwrap();
    assert_eq!(model.kv_cache().unwrap().capacity(), 512);
}
