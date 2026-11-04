//! [`Model::decode_batch`] (ADR 0013 spike S13-2): several sequences decoded
//! together give each one's own bits, in every tier and quant mix.

extern crate std;

use crate::fixture::{self, Spec};
use crate::*;
use alloc::vec;
use alloc::vec::Vec;
use gguf::Gguf;
use quant::Tier;

/// Prompts of different lengths, so the sequences sit at different positions.
const PROMPTS: [&[u32]; 4] = [&[1, 2, 3], &[7, 8, 9, 10, 11], &[4], &[20, 21]];

/// Each sequence's logits for `steps` greedy tokens after its prompt, run alone
/// with [`Model::forward`], or together with [`Model::decode_batch`].
fn decode(spec: &Spec, tier: Tier, count: usize, steps: usize, batched: bool) -> Vec<Vec<u32>> {
    let bytes = fixture::build(spec);
    let file = Gguf::parse(&bytes).unwrap();
    let mut model = Model::new(&file, tier, 64).unwrap();
    let vocab = model.config().vocab;
    let mut kvs: Vec<KvCache> = (0..count).map(|_| model.kv_cache().unwrap()).collect();
    let mut next: Vec<u32> = Vec::new();
    let mut logits = vec![0.0f32; vocab];
    for (kv, prompt) in kvs.iter_mut().zip(PROMPTS) {
        model.forward_batch(kv, prompt, Some(&mut logits)).unwrap();
        next.push(generate::argmax(&logits));
    }
    let mut bits = vec![Vec::new(); count];
    for _ in 0..steps {
        let mut all = vec![0.0f32; count * vocab];
        if batched {
            let mut refs: Vec<&mut KvCache> = kvs.iter_mut().collect();
            model.decode_batch(&mut refs, &next, &mut all).unwrap();
        } else {
            for (i, kv) in kvs.iter_mut().enumerate() {
                model
                    .forward(kv, next[i], Some(&mut all[i * vocab..][..vocab]))
                    .unwrap();
            }
        }
        for (i, one) in all.chunks_exact(vocab).enumerate() {
            bits[i].extend(one.iter().map(|v| v.to_bits()));
            next[i] = generate::argmax(one);
        }
    }
    bits
}

#[test]
fn a_batch_decodes_each_sequence_to_its_own_bits() {
    for spec in [Spec::tiny(), Spec::tiny_k()] {
        for tier in Tier::all() {
            for count in [1, 2, 4] {
                assert!(
                    decode(&spec, tier, count, 6, true) == decode(&spec, tier, count, 6, false),
                    "{} sequences in {} differ",
                    count,
                    tier.name()
                );
            }
        }
    }
}

#[test]
fn a_batch_refuses_what_a_forward_would() {
    let bytes = fixture::build(&Spec::tiny());
    let file = Gguf::parse(&bytes).unwrap();
    let mut model = Model::new(&file, Tier::best(), 4).unwrap();
    let vocab = model.config().vocab;
    let (mut a, mut b) = (model.kv_cache().unwrap(), model.kv_cache().unwrap());
    let mut logits = vec![0.0f32; 2 * vocab];
    // One token a sequence, one vocabulary of logits each.
    assert!(
        model
            .decode_batch(&mut [&mut a], &[1, 2], &mut logits)
            .is_err()
    );
    assert!(
        model
            .decode_batch(&mut [&mut a, &mut b], &[1, 2], &mut logits[..vocab])
            .is_err()
    );
    // A full cache refuses, and nothing changes.
    model.forward_batch(&mut a, &[1, 2, 3, 4], None).unwrap();
    assert!(matches!(
        model.decode_batch(&mut [&mut a, &mut b], &[1, 2], &mut logits),
        Err(Error::ContextFull)
    ));
    assert_eq!((a.len(), b.len()), (4, 0));
}
