//! Batched prefill: a prompt run in blocks gives the logits and the KV cache of
//! running it one token at a time, bit for bit, on any pool.

use crate::fixture::{self, Spec};
use crate::kquant_tests::Threads;
use crate::*;
use alloc::vec;
use alloc::vec::Vec;
use gguf::Gguf;
use quant::{Parallel, Serial, Tier};

/// A prompt of `len` tokens below the vocabulary.
fn tokens(len: usize, vocab: usize) -> Vec<u32> {
    (0..len).map(|i| ((i * 37 + 11) % vocab) as u32).collect()
}

/// The logits after the last token, and every cached half, from feeding `first`
/// and then `then` with `feed`.
fn run(
    bytes: &[u8],
    (tier, par): (Tier, &dyn Parallel),
    (first, then): (&[u32], &[u32]),
    batched: bool,
) -> (Vec<u32>, Vec<u16>) {
    let file = Gguf::parse(bytes).unwrap();
    let mut model = Model::new(&file, tier, 160).unwrap();
    model.set_parallel(par);
    let mut kv = model.kv_cache().unwrap();
    let mut logits = vec![0.0f32; model.config().vocab];
    for part in [first, then] {
        if batched {
            model
                .forward_batch(&mut kv, part, Some(&mut logits))
                .unwrap();
        } else {
            for &token in part {
                model.forward(&mut kv, token, Some(&mut logits)).unwrap();
            }
        }
    }
    let mut halves = Vec::new();
    for layer in 0..model.config().n_layer {
        let (keys, values) = kv.layer(layer, kv.len() - 1);
        halves.extend_from_slice(keys);
        halves.extend_from_slice(values);
    }
    (logits.iter().map(|v| v.to_bits()).collect(), halves)
}

#[test]
fn blocks_give_the_bits_of_one_token_at_a_time() {
    for spec in [Spec::tiny(), Spec::tiny_k()] {
        let bytes = fixture::build(&spec);
        // Past two blocks, then a short run on top of a cached start.
        let prompt = tokens(2 * PREFILL_TOKENS + 7, spec.vocab);
        let more = tokens(5, spec.vocab);
        let parts = (&prompt[..], &more[..]);
        // One token at a time in the baseline tier is the reference; blocks
        // run the batched kernels of every tier the CPU has, on one thread and
        // on three.
        let want = run(&bytes, (Tier::BASELINE, &Serial), parts, false);
        for tier in Tier::all() {
            for pool in [&Serial as &dyn Parallel, &Threads(3)] {
                let got = run(&bytes, (tier, pool), parts, true);
                assert!(got == want, "seed {}, {}", spec.seed, tier.name());
            }
        }
    }
}

#[test]
fn a_bad_batch_leaves_the_cache_as_it_was() {
    let bytes = fixture::build(&Spec::tiny());
    let file = Gguf::parse(&bytes).unwrap();
    let mut model = Model::new(&file, Tier::best(), 48).unwrap();
    let mut kv = model.kv_cache().unwrap();
    model.forward_batch(&mut kv, &[1, 2, 3], None).unwrap();
    assert_eq!(kv.len(), 3);
    assert_eq!(
        model.forward_batch(&mut kv, &[], None),
        Err(Error::EmptyPrompt)
    );
    assert_eq!(
        model.forward_batch(&mut kv, &[4, 384, 5], None),
        Err(Error::Token(384))
    );
    let long = tokens(46, 384);
    assert_eq!(
        model.forward_batch(&mut kv, &long, None),
        Err(Error::ContextFull)
    );
    let mut short = [0.0f32; 3];
    assert_eq!(
        model.forward_batch(&mut kv, &[4], Some(&mut short)),
        Err(Error::Logits)
    );
    assert_eq!(kv.len(), 3);
    // The last position fits exactly.
    model.forward_batch(&mut kv, &long[..45], None).unwrap();
    assert_eq!(kv.len(), 48);
}
