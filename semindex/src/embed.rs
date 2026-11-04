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

/// Turns text into a vector, as S13-3's embedder will. A document and a query
/// are embedded apart because Qwen3-Embedding gives a query its instruction.
/// Each answer must have [`EmbedderId::dims`] values; it is made unit length
/// before it is kept, so the scan's dot product is the cosine.
pub trait Embedder {
    fn id(&self) -> EmbedderId;
    fn document(&self, text: &str) -> Result<Vec<f32>, String>;
    fn query(&self, text: &str) -> Result<Vec<f32>, String>;
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
