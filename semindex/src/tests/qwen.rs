//! The Qwen3 embedder over `infer`, on a generated last-token-pooling model:
//! what it embeds, a `kai-index 1` written with its vectors and read back, and
//! an index of another embedder's vectors refused for a rebuild.

use super::{CORPUS, corpus_index};
use crate::embed::{EmbedError, Embedder, EmbedderId, unit};
use crate::format::{Header, Refused, open};
use crate::qwen::{BpeTokens, DIMS, NAME, QUERY_INSTRUCTION, Qwen3Embedder, Unusable};
use crate::scope::Scope;
use crate::search::{Query, search};
use crate::{Tokens, chunk, half};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use bpe::Tokenizer;
use gguf::Gguf;
use infer::fixture::{self, Spec};
use infer::{Model, Pooling};
use quant::Tier;

const SHA: [u8; 32] = [0x5e; 32];
const CONTEXT: usize = 512;

/// A model wider than [`DIMS`], so the cut keeps part of each vector.
fn spec() -> Spec {
    Spec {
        n_embd: 384,
        pooling_type: Some(3),
        seed: 13,
        ..Spec::tiny()
    }
}

/// The fixture's file and its tokenizer, which the embedder borrows.
struct Fixture {
    bytes: Vec<u8>,
}

impl Fixture {
    fn new(spec: &Spec) -> Self {
        Self {
            bytes: fixture::build(spec),
        }
    }

    fn file(&self) -> Gguf<'_> {
        Gguf::parse(&self.bytes).unwrap()
    }

    fn tokenizer(&self) -> Tokenizer {
        Tokenizer::from_gguf(self.file().header()).unwrap()
    }
}

fn embedder<'a>(file: &Gguf<'a>, tok: &'a Tokenizer) -> Qwen3Embedder<'a> {
    let model = Model::new(file, Tier::best(), CONTEXT).unwrap();
    Qwen3Embedder::new(model, tok, SHA, DIMS).unwrap()
}

fn document(e: &mut Qwen3Embedder<'_>, text: &str) -> Vec<f32> {
    let mut out = vec![0.0; usize::from(DIMS)];
    e.embed_document(text, &mut out).unwrap();
    out
}

#[test]
fn a_vector_is_the_models_last_token_state_cut_to_256() {
    let fx = Fixture::new(&spec());
    let (file, tok) = (fx.file(), fx.tokenizer());
    let mut e = embedder(&file, &tok);
    let text = "Notes from the budget meeting";
    let ours = document(&mut e, text);
    // The engine's own vector for the text and `<|endoftext|>`, by hand.
    let mut model = Model::new(&file, Tier::best(), CONTEXT).unwrap();
    assert_eq!(model.config().pooling, Pooling::Last);
    let mut ids = Vec::new();
    tok.encode(text, &mut ids);
    ids.push(tok.special("<|endoftext|>").unwrap());
    let mut kv = model.kv_cache().unwrap();
    let mut full = vec![0.0; spec().n_embd];
    model.embed(&mut kv, &ids, &mut full).unwrap();
    assert_eq!(ours, full[..usize::from(DIMS)]);
    // From an empty cache each time: the same text gives the same vector.
    assert_eq!(document(&mut e, "something else entirely").len(), 256);
    assert_eq!(document(&mut e, text), ours);
    assert_eq!(
        e.id(),
        EmbedderId {
            name: String::from(NAME),
            sha256: SHA,
            dims: DIMS,
        }
    );
}

#[test]
fn a_query_is_embedded_after_the_query_instruction() {
    let fx = Fixture::new(&spec());
    let (file, tok) = (fx.file(), fx.tokenizer());
    let mut e = embedder(&file, &tok);
    let mut query = vec![0.0; usize::from(DIMS)];
    e.embed_query("the budget meeting", &mut query).unwrap();
    let instructed = document(
        &mut e,
        &alloc::format!("{QUERY_INSTRUCTION}the budget meeting"),
    );
    assert_eq!(query, instructed);
    assert_ne!(query, document(&mut e, "the budget meeting"));
}

#[test]
fn a_text_past_the_context_or_a_wrong_buffer_gives_no_vector() {
    let fx = Fixture::new(&spec());
    let (file, tok) = (fx.file(), fx.tokenizer());
    let mut e = embedder(&file, &tok);
    let long = "word ".repeat(CONTEXT);
    let mut out = vec![0.0; usize::from(DIMS)];
    assert_eq!(e.embed_document(&long, &mut out), Err(EmbedError::TooLong));
    let mut short = vec![0.0; 16];
    assert_eq!(e.embed_document("hi", &mut short), Err(EmbedError::Model));
    // It still embeds after either.
    assert!(e.embed_document("hi", &mut out).is_ok());
}

#[test]
fn only_a_pooling_model_with_room_for_the_cut_is_an_embedder() {
    let chat = Fixture::new(&Spec {
        pooling_type: None,
        ..spec()
    });
    let (file, tok) = (chat.file(), chat.tokenizer());
    let model = || Model::new(&file, Tier::best(), CONTEXT).unwrap();
    assert!(matches!(
        Qwen3Embedder::new(model(), &tok, SHA, DIMS),
        Err(Unusable::NotPooled)
    ));
    let fx = Fixture::new(&spec());
    let (file, tok) = (fx.file(), fx.tokenizer());
    let model = || Model::new(&file, Tier::best(), CONTEXT).unwrap();
    for dims in [0, 385] {
        assert!(matches!(
            Qwen3Embedder::new(model(), &tok, SHA, dims),
            Err(Unusable::Dims)
        ));
    }
    assert!(Qwen3Embedder::new(model(), &tok, SHA, 384).is_ok());
}

#[test]
fn the_chunker_counts_the_models_tokens() {
    let fx = Fixture::new(&spec());
    let tok = fx.tokenizer();
    let tokens = BpeTokens(&tok);
    let mut ids = Vec::new();
    tok.encode("the budget meeting", &mut ids);
    assert_eq!(tokens.count("the budget meeting"), ids.len());
    let text = "plan the quarterly budget ".repeat(200);
    let chunks = chunk::chunk(&text, &tokens);
    assert!(chunks.len() > 1);
    // The chunker counts word by word, by the model's tokens: about 400 a chunk.
    let words = |c: &crate::Chunk| -> usize {
        c.of(&text)
            .split_whitespace()
            .map(|w| tokens.count(w))
            .sum()
    };
    assert!(chunks.iter().all(|c| words(c) <= 400));
    assert!(words(&chunks[0]) > 360, "{}", words(&chunks[0]));
}

/// Each chunk's stored vector is its text's vector at unit length in f16.
#[test]
fn an_index_with_its_vectors_round_trips_through_kai_index_1() {
    let fx = Fixture::new(&spec());
    let (file, tok) = (fx.file(), fx.tokenizer());
    let mut e = embedder(&file, &tok);
    let (index, _) = corpus_index(Some(&mut e));
    assert_eq!(index.header.embedder, Some(e.id()));
    let mut vectors = 0;
    for entry in index.files() {
        let doc = CORPUS.iter().find(|d| d.path == entry.path).unwrap();
        for c in &entry.chunks {
            let words = &doc.text[c.offset as usize..][..c.len as usize];
            let want: Vec<u16> = unit(&document(&mut e, words))
                .unwrap()
                .into_iter()
                .map(half::narrow)
                .collect();
            assert_eq!(c.vector.as_ref(), Some(&want), "{}", entry.path);
            vectors += 1;
        }
    }
    assert!(vectors >= 5, "{vectors} vectors");
    let query = Query {
        text: "milk and eggs",
        ..Query::default()
    };
    let hits = search(&index, &Scope::new(), &query, Some(&mut e)).unwrap();
    assert!(hits.iter().any(|h| h.path.ends_with("groceries.txt")));
}

#[test]
fn an_index_of_another_embedders_vectors_is_rebuilt() {
    let fx = Fixture::new(&spec());
    let (file, tok) = (fx.file(), fx.tokenizer());
    let mut e = embedder(&file, &tok);
    let (_, bytes) = corpus_index(Some(&mut e));
    let ours = e.id();
    assert!(
        open(
            &bytes,
            &Header {
                embedder: Some(ours.clone())
            }
        )
        .is_ok()
    );
    let others = [
        EmbedderId {
            sha256: [0x6f; 32],
            ..ours.clone()
        },
        EmbedderId {
            dims: 128,
            ..ours.clone()
        },
        EmbedderId {
            name: String::from("qwen3-embedding-4b"),
            ..ours.clone()
        },
    ];
    for other in others {
        let want = Header {
            embedder: Some(other),
        };
        assert_eq!(open(&bytes, &want).err(), Some(Refused::Embedder));
    }
    let keyword_only = Header { embedder: None };
    assert_eq!(open(&bytes, &keyword_only).err(), Some(Refused::Embedder));
}
