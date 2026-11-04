//! Last-token pooling (ADR 0013 S13-3): [`Model::embed`] against the naive
//! reference's final hidden state, on generated models.

extern crate std;

use crate::fixture::{self, Spec};
use crate::reference::Reference;
use crate::*;
use alloc::vec;
use alloc::vec::Vec;
use gguf::Gguf;
use quant::Tier;

fn tokens(bytes: &[u8], text: &str) -> Vec<u32> {
    let file = Gguf::parse(bytes).unwrap();
    let tok = bpe::Tokenizer::from_gguf(file.header()).unwrap();
    let mut ids = Vec::new();
    tok.encode(text, &mut ids);
    ids
}

fn unit(v: &[f32]) -> Vec<f32> {
    let norm = libm::sqrtf(v.iter().map(|x| x * x).sum());
    v.iter().map(|x| x / norm).collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

fn embedding_spec() -> Spec {
    Spec {
        pooling_type: Some(3),
        seed: 7,
        ..Spec::tiny()
    }
}

#[test]
fn the_embedding_is_the_references_last_hidden_state() {
    for spec in [
        embedding_spec(),
        Spec {
            n_layer: 3,
            seed: 8,
            ..embedding_spec()
        },
    ] {
        let bytes = fixture::build(&spec);
        let file = Gguf::parse(&bytes).unwrap();
        let mut model = Model::new(&file, Tier::best(), 128).unwrap();
        assert_eq!(model.config().pooling, Pooling::Last);
        let ids = tokens(&bytes, "Notes from the Leeds office budget meeting");
        let mut kv = model.kv_cache().unwrap();
        let mut ours = vec![0.0f32; spec.n_embd];
        model.embed(&mut kv, &ids, &mut ours).unwrap();
        let mut reference = Reference::new(&bytes);
        let mut hidden = Vec::new();
        for &id in &ids {
            hidden = reference.hidden_on(id, Some(&kv));
        }
        let want = unit(&hidden);
        let similarity = cosine(&ours, &want);
        assert!(
            similarity > 0.9999,
            "seed {}: cosine {similarity}",
            spec.seed
        );
        let length: f32 = ours.iter().map(|v| v * v).sum();
        assert!((length - 1.0).abs() < 1e-5, "length {length}");
    }
}

#[test]
fn the_bits_do_not_depend_on_how_the_tokens_are_fed() {
    let spec = embedding_spec();
    let bytes = fixture::build(&spec);
    let file = Gguf::parse(&bytes).unwrap();
    let mut model = Model::new(&file, Tier::best(), spec.context_length).unwrap();
    // Longer than one prefill block, so the last block is a short one.
    let ids = tokens(
        &bytes,
        "an index of files and notes, chunked and embedded once",
    );
    assert!(ids.len() > PREFILL_TOKENS && !ids.len().is_multiple_of(PREFILL_TOKENS));
    let mut whole = vec![0.0f32; spec.n_embd];
    let mut kv = model.kv_cache().unwrap();
    model.embed(&mut kv, &ids, &mut whole).unwrap();
    let mut split = vec![0.0f32; spec.n_embd];
    let mut kv = model.kv_cache().unwrap();
    let (head, tail) = ids.split_at(ids.len() - 1);
    model.forward_batch(&mut kv, head, None).unwrap();
    model.embed(&mut kv, tail, &mut split).unwrap();
    assert_eq!(whole, split);
    // Every tier gives the same bits, as the logits do.
    for tier in Tier::all() {
        let mut model = Model::new(&file, tier, spec.context_length).unwrap();
        let mut kv = model.kv_cache().unwrap();
        let mut again = vec![0.0f32; spec.n_embd];
        model.embed(&mut kv, &ids, &mut again).unwrap();
        assert_eq!(whole, again, "{tier:?}");
    }
}

#[test]
fn a_wrong_buffer_an_empty_text_and_other_poolings_are_refused() {
    let spec = embedding_spec();
    let bytes = fixture::build(&spec);
    let file = Gguf::parse(&bytes).unwrap();
    let mut model = Model::new(&file, Tier::best(), 128).unwrap();
    let mut kv = model.kv_cache().unwrap();
    let mut short = vec![0.0f32; spec.n_embd - 1];
    assert_eq!(
        model.embed(&mut kv, &[1, 2], &mut short),
        Err(Error::Embedding)
    );
    let mut out = vec![0.0f32; spec.n_embd];
    assert_eq!(model.embed(&mut kv, &[], &mut out), Err(Error::EmptyPrompt));
    // Neither refusal touched the cache.
    assert!(kv.is_empty());
    // A chat model has no pooling; mean pooling (1) is not done.
    let chat = fixture::build(&Spec::tiny());
    let chat = Model::new(&Gguf::parse(&chat).unwrap(), Tier::best(), 128).unwrap();
    assert_eq!(chat.config().pooling, Pooling::None);
    let mean = fixture::build(&Spec {
        pooling_type: Some(1),
        ..spec
    });
    assert_eq!(
        Model::new(&Gguf::parse(&mean).unwrap(), Tier::best(), 128).err(),
        Some(Error::Unsupported("pooling other than the last token's"))
    );
}
