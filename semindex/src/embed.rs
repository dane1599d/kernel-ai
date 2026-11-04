//! The vector slot (ADR 0013 section 4): S13-3's embedder, Qwen3-Embedding-0.6B
//! run by our engine, is one [`Embedder`]; until it passes, an index has no
//! vectors and search is by keyword and time. An index names the embedder its
//! vectors came from ([`EmbedderId`], in its header), and one opened with
//! another is rebuilt, never read, since their vectors do not compare.

use alloc::string::String;
use alloc::vec::Vec;

/// Most dimensions a vector may have (the ADR keeps 256).
pub const MAX_DIMS: usize = 1024;
/// The longest embedder name kept, in bytes.
pub const MAX_NAME: usize = 64;

/// Which embedder made an index's vectors: its name, its model file's SHA-256
/// and the dimensions it gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbedderId {
    pub name: String,
    pub sha256: [u8; 32],
    pub dims: u16,
}

/// Why an embedder gave no vector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbedError {
    /// The text is longer than the model takes.
    TooLong,
    /// The model failed.
    Model,
}

/// Turns text into a vector, as S13-3's embedder does on `infer` (the shape
/// agreed with the P5 spikes lane): it needs `&mut self` for the engine's
/// state, and writes into the caller's buffer. A document and a query are
/// embedded apart because Qwen3-Embedding gives a query its instruction. `out`
/// has [`EmbedderId::dims`] values; what is written is made unit length again
/// before it is kept, so the scan's dot product is the cosine. A chunk with no
/// vector is found by keyword, and a query with none falls back to keyword.
/// Tokens are counted apart, by [`crate::Tokens`], so the chunker borrows the
/// tokenizer while the engine is borrowed to embed.
pub trait Embedder {
    fn id(&self) -> EmbedderId;
    fn embed_document(&mut self, text: &str, out: &mut [f32]) -> Result<(), EmbedError>;
    fn embed_query(&mut self, text: &str, out: &mut [f32]) -> Result<(), EmbedError>;
}

/// `v` at unit length, or `None` for one of zero length or with a value that
/// is not finite.
pub fn unit(v: &[f32]) -> Option<Vec<f32>> {
    if v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    let length = libm::sqrtf(v.iter().map(|x| x * x).sum::<f32>());
    (length > 0.0 && length.is_finite()).then(|| v.iter().map(|x| x / length).collect())
}

/// The dot product of two vectors of one length; 0 for two of different
/// lengths, which an index never holds.
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
