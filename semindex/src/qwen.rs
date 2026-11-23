//! The index's embedder (ADR 0013 section 4, spike S13-3): Qwen3-Embedding-0.6B
//! run by `infer`, as S13-3 measured it against llama.cpp. A text's tokens end
//! with `<|endoftext|>` (the file's `add_eos_token`), the last token's hidden
//! state is the vector (`infer::Model::embed`, last-token pooling), a query is
//! embedded after the model's query instruction, and the vector is cut to its
//! first [`DIMS`] values (MRL). The index makes it unit length again and keeps
//! it as f16 ([`crate::half`]).
//!
//! The tokenizer is borrowed, not owned, so the chunker counts with
//! [`BpeTokens`] while the engine is borrowed to embed.

use crate::chunk::Tokens;
use crate::embed::{EmbedError, Embedder, EmbedderId, MAX_DIMS};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use bpe::Tokenizer;
use infer::{KvCache, Model, Pooling};

/// What [`EmbedderId::name`] says: the catalog id of the pinned model
/// (`xtask/models.json`).
pub const NAME: &str = "qwen3-embedding-0.6b";
/// The dimensions `search` keeps (ADR 0013 section 4).
pub const DIMS: u16 = 256;
/// The query instruction S13-3 measured with (Qwen3-Embedding's
/// `Instruct: {task}\nQuery:{query}`): a document gets none.
pub const QUERY_INSTRUCTION: &str =
    "Instruct: Given a search query, retrieve the notes and files that answer it\nQuery:";
/// The token that ends every embedded text.
const END_OF_TEXT: &str = "<|endoftext|>";

/// The chunker's token count, by the model's own tokenizer.
pub struct BpeTokens<'t>(pub &'t Tokenizer);

impl Tokens for BpeTokens<'_> {
    fn count(&self, text: &str) -> usize {
        let mut ids = Vec::new();
        self.0.encode(text, &mut ids);
        ids.len()
    }
}

/// Why a model cannot be the index's embedder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unusable {
    /// The file does not pool its last token (`qwen3.pooling_type` 3), so it
    /// is not an embedding model.
    NotPooled,
    /// No dimensions, more than the model gives, or more than an index keeps.
    Dims,
    /// The vocabulary has no `<|endoftext|>`.
    NoEndOfText,
    /// The engine could not make the KV cache.
    Engine(infer::Error),
}

pub struct Qwen3Embedder<'a> {
    model: Model<'a>,
    kv: KvCache,
    tok: &'a Tokenizer,
    eos: u32,
    id: EmbedderId,
    /// The model's whole vector, before the cut.
    full: Vec<f32>,
    ids: Vec<u32>,
}

impl<'a> Qwen3Embedder<'a> {
    /// The embedder over `model`, with `tok` from the same file, whose
    /// SHA-256 is `sha256`, giving `dims` values a vector ([`DIMS`] for
    /// `search`). The context `model` was made with bounds a text's tokens.
    pub fn new(
        model: Model<'a>,
        tok: &'a Tokenizer,
        sha256: [u8; 32],
        dims: u16,
    ) -> Result<Self, Unusable> {
        let config = model.config();
        if config.pooling != Pooling::Last {
            return Err(Unusable::NotPooled);
        }
        let width = config.n_embd;
        if dims == 0 || usize::from(dims) > width || usize::from(dims) > MAX_DIMS {
            return Err(Unusable::Dims);
        }
        let eos = tok.special(END_OF_TEXT).ok_or(Unusable::NoEndOfText)?;
        let kv = model.kv_cache().map_err(Unusable::Engine)?;
        Ok(Self {
            model,
            kv,
            tok,
            eos,
            id: EmbedderId {
                name: String::from(NAME),
                sha256,
                dims,
            },
            full: vec![0.0; width],
            ids: Vec::new(),
        })
    }

    /// The first `out.len()` values of `text`'s vector: `text`'s tokens and
    /// `<|endoftext|>`, run from an empty cache.
    fn run(&mut self, text: &str, out: &mut [f32]) -> Result<(), EmbedError> {
        if out.len() != usize::from(self.id.dims) {
            return Err(EmbedError::Model);
        }
        self.ids.clear();
        self.tok.encode(text, &mut self.ids);
        self.ids.push(self.eos);
        if self.ids.len() > self.kv.capacity() {
            return Err(EmbedError::TooLong);
        }
        self.kv.clear();
        self.model
            .embed(&mut self.kv, &self.ids, &mut self.full)
            .map_err(|_| EmbedError::Model)?;
        out.copy_from_slice(&self.full[..out.len()]);
        Ok(())
    }
}

impl Embedder for Qwen3Embedder<'_> {
    fn id(&self) -> EmbedderId {
        self.id.clone()
    }

    fn embed_document(&mut self, text: &str, out: &mut [f32]) -> Result<(), EmbedError> {
        self.run(text, out)
    }

    /// The instruction and the query are tokenized as one text, as llama.cpp
    /// was given them in S13-3: `Query:` and the query's first word may share
    /// a token.
    fn embed_query(&mut self, text: &str, out: &mut [f32]) -> Result<(), EmbedError> {
        self.run(&format!("{QUERY_INSTRUCTION}{text}"), out)
    }
}
