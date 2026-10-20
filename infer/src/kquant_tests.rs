//! The forward pass on Q4_K and Q6_K weights, a Q4_K_M file's mix: against the
//! naive reference, in every tier, and on several threads.

extern crate std;

use crate::fixture::{self, Spec};
use crate::reference::Reference;
use crate::*;
use alloc::vec;
use alloc::vec::Vec;
use gguf::{Gguf, TensorType};
use quant::{Parallel, Tier};

/// `workers` OS threads, started afresh for every broadcast.
pub(crate) struct Threads(pub usize);

impl Parallel for Threads {
    fn workers(&self) -> usize {
        self.0
    }

    fn broadcast(&self, job: &(dyn Fn(usize, usize) + Sync)) {
        std::thread::scope(|scope| {
            for worker in 0..self.0 {
                scope.spawn(move || job(worker, self.0));
            }
        });
    }
}

fn prompt(bytes: &[u8], question: &str) -> Vec<u32> {
    let file = Gguf::parse(bytes).unwrap();
    let tok = bpe::Tokenizer::from_gguf(file.header()).unwrap();
    let chat = bpe::chat::Chat::new(&tok).unwrap();
    chat.prompt(&tok, None, question)
}

/// The logits after every prompt token and 16 greedy ones, as bits.
fn logits_bits(bytes: &[u8], tier: Tier, par: &dyn Parallel) -> Vec<u32> {
    let file = Gguf::parse(bytes).unwrap();
    let mut model = Model::new(&file, tier, 128).unwrap();
    model.set_parallel(par);
    let mut kv = model.kv_cache().unwrap();
    let mut logits = vec![0.0f32; model.config().vocab];
    let mut bits = Vec::new();
    let mut tokens = prompt(bytes, "same bits?");
    for i in 0..tokens.len() + 16 {
        model
            .forward(&mut kv, tokens[i], Some(&mut logits))
            .unwrap();
        bits.extend(logits.iter().map(|v| v.to_bits()));
        if i + 1 >= tokens.len() {
            tokens.push(argmax(&logits));
        }
    }
    bits
}

fn specs() -> [Spec; 2] {
    let k = Spec::tiny_k();
    [
        k,
        Spec {
            untied: true,
            n_layer: 3,
            n_head_kv: 1,
            seed: 22,
            ..k
        },
    ]
}

#[test]
fn the_fixture_mixes_q4_k_and_q6_k() {
    let bytes = fixture::build(&Spec::tiny_k());
    let file = Gguf::parse(&bytes).unwrap();
    let ty = |name: &str| file.tensor(name).unwrap().info.ty;
    assert_eq!(ty("token_embd.weight"), TensorType::Q6_K);
    assert_eq!(ty("blk.0.attn_v.weight"), TensorType::Q6_K);
    assert_eq!(ty("blk.1.attn_v.weight"), TensorType::Q4_K);
    assert_eq!(ty("blk.0.ffn_down.weight"), TensorType::Q6_K);
    assert_eq!(ty("blk.0.attn_q.weight"), TensorType::Q4_K);
}

#[test]
fn k_quant_logits_match_the_reference() {
    for spec in specs() {
        let bytes = fixture::build(&spec);
        let file = Gguf::parse(&bytes).unwrap();
        let mut model = Model::new(&file, Tier::best(), 128).unwrap();
        let mut kv = model.kv_cache().unwrap();
        let mut reference = Reference::new(&bytes);
        let mut logits = vec![0.0f32; spec.vocab];
        let mut tokens = prompt(&bytes, "What does a kernel do?");
        let (mut made, mut close) = (Vec::new(), 0);
        let steps = tokens.len() + 24;
        for i in 0..steps {
            model
                .forward(&mut kv, tokens[i], Some(&mut logits))
                .unwrap();
            let want = reference.forward(tokens[i]);
            let scale = want.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let diff = logits
                .iter()
                .zip(&want)
                .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
            // At these widths an activation now and then lands within float
            // rounding of a step boundary, the two round it apart, and the
            // logits move by about 2% from then on (Q8_0 at the same widths does
            // the same). A wiring mistake moves them by far more; exact bits are
            // the tier and thread tests' job.
            let error = diff / scale;
            assert!(error < 0.05, "seed {}: error {error} at {i}", spec.seed);
            close += usize::from(error < 1e-3);
            if i + 1 >= tokens.len() {
                // Both are fed the engine's pick, so one differing pick does not
                // set them on different texts.
                made.push(argmax(&logits));
                tokens.push(argmax(&logits));
            }
        }
        assert!(close >= 5, "seed {}: only {close} close", spec.seed);
        made.sort_unstable();
        made.dedup();
        assert!(made.len() >= 2, "seed {}: only {made:?}", spec.seed);
    }
}

#[test]
fn every_tier_and_any_threads_give_the_same_k_quant_bits() {
    for spec in specs() {
        let bytes = fixture::build(&spec);
        let baseline = logits_bits(&bytes, Tier::BASELINE, &quant::Serial);
        for tier in Tier::all() {
            assert!(
                logits_bits(&bytes, tier, &quant::Serial) == baseline,
                "{} differs from baseline",
                tier.name()
            );
        }
        for workers in [2, 3, 8] {
            assert!(
                logits_bits(&bytes, Tier::best(), &Threads(workers)) == baseline,
                "{workers} threads differ from one"
            );
        }
    }
}

#[test]
fn q8_0_on_threads_gives_one_thread_bits() {
    let bytes = fixture::build(&Spec::tiny());
    let one = logits_bits(&bytes, Tier::best(), &quant::Serial);
    assert!(logits_bits(&bytes, Tier::best(), &Threads(4)) == one);
}
