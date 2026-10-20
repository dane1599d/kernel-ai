//! A qwen3 model's shape, from its GGUF metadata, checked before anything is sized
//! from it.

use crate::Error;
use gguf::Header;

/// Bounds on each dimension. They hold every Qwen3 model up to 235B with room to
/// spare, and keep products such as the KV cache size far from overflow.
const MAX_LAYERS: usize = 1024;
const MAX_WIDTH: usize = 1 << 17;
const MAX_HEADS: usize = 1024;
const MAX_HEAD_DIM: usize = 1024;
pub const MAX_VOCAB: usize = 1 << 24;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    pub n_layer: usize,
    /// The residual stream's width.
    pub n_embd: usize,
    pub n_ff: usize,
    pub n_head: usize,
    pub n_head_kv: usize,
    pub head_dim: usize,
    pub vocab: usize,
    /// The context the model was trained for.
    pub context_length: usize,
    pub rope_theta: f32,
    pub rms_eps: f32,
}

impl Config {
    /// Reads `qwen3.*` metadata and the vocabulary size from `token_embd.weight`.
    pub fn from_gguf(header: &Header<'_>) -> Result<Self, Error> {
        let arch = header.get("general.architecture").and_then(|v| v.as_str());
        if arch != Some("qwen3") {
            return Err(Error::Architecture);
        }
        rope_unscaled(header)?;
        if let Some((key, _)) = header.metadata().find(|(key, _)| !known_key(key)) {
            return Err(Error::UnknownKey(crate::weights::shown(key)));
        }
        let int = |key: &'static str, max: usize| -> Result<usize, Error> {
            let value = header.get(key).ok_or(Error::MissingKey(key))?;
            match value.as_u64().and_then(|v| usize::try_from(v).ok()) {
                Some(v) if (1..=max).contains(&v) => Ok(v),
                _ => Err(Error::BadValue(key)),
            }
        };
        let float = |key: &'static str, default: Option<f32>| -> Result<f32, Error> {
            match header.get(key) {
                None => default.ok_or(Error::MissingKey(key)),
                Some(value) => match value.as_f32() {
                    Some(v) if v.is_finite() && v > 0.0 => Ok(v),
                    _ => Err(Error::BadValue(key)),
                },
            }
        };
        let n_embd = int("qwen3.embedding_length", MAX_WIDTH)?;
        let n_head = int("qwen3.attention.head_count", MAX_HEADS)?;
        let head_dim = match header.get("qwen3.attention.key_length") {
            Some(_) => int("qwen3.attention.key_length", MAX_HEAD_DIM)?,
            None => n_embd / n_head,
        };
        if header.get("qwen3.attention.value_length").is_some()
            && int("qwen3.attention.value_length", MAX_HEAD_DIM)? != head_dim
        {
            return Err(Error::BadValue("qwen3.attention.value_length"));
        }
        // llama.cpp rotates only `rope.dimension_count` dims of each head when the
        // key is there; this engine rotates whole heads.
        if header.get("qwen3.rope.dimension_count").is_some()
            && int("qwen3.rope.dimension_count", MAX_HEAD_DIM)? != head_dim
        {
            return Err(Error::Unsupported("partial rotary embeddings"));
        }
        let vocab = header
            .tensor("token_embd.weight")
            .and_then(|t| t.dims().get(1).copied())
            .and_then(|v| usize::try_from(v).ok())
            .filter(|v| (1..=MAX_VOCAB).contains(v))
            .ok_or(Error::BadValue("token_embd.weight"))?;
        let config = Self {
            n_layer: int("qwen3.block_count", MAX_LAYERS)?,
            n_embd,
            n_ff: int("qwen3.feed_forward_length", MAX_WIDTH)?,
            n_head,
            n_head_kv: int("qwen3.attention.head_count_kv", MAX_HEADS)?,
            head_dim,
            vocab,
            context_length: int("qwen3.context_length", 1 << 24)?,
            rope_theta: float("qwen3.rope.freq_base", Some(10_000.0))?,
            rms_eps: float("qwen3.attention.layer_norm_rms_epsilon", None)?,
        };
        config.check()?;
        Ok(config)
    }

    /// The rules the forward pass relies on.
    fn check(&self) -> Result<(), Error> {
        let block = quant::BLOCK_LEN;
        if !self.n_head.is_multiple_of(self.n_head_kv) {
            return Err(Error::BadValue("qwen3.attention.head_count_kv"));
        }
        if self.head_dim == 0 || !self.head_dim.is_multiple_of(2) || self.head_dim > MAX_HEAD_DIM {
            return Err(Error::BadValue("qwen3.attention.key_length"));
        }
        // Q8_0 rows: every matrix's input is a whole number of blocks.
        if !self.n_embd.is_multiple_of(block) {
            return Err(Error::BadValue("qwen3.embedding_length"));
        }
        if !self.n_ff.is_multiple_of(block) {
            return Err(Error::BadValue("qwen3.feed_forward_length"));
        }
        if !self.q_dim().is_multiple_of(block) {
            return Err(Error::BadValue("qwen3.attention.head_count"));
        }
        // Already finite and positive; a norm epsilon of 1 or more is no epsilon.
        if self.rms_eps >= 1.0 {
            return Err(Error::BadValue("qwen3.attention.layer_norm_rms_epsilon"));
        }
        Ok(())
    }

    /// Width of the queries: one head_dim per query head.
    pub fn q_dim(&self) -> usize {
        self.n_head * self.head_dim
    }

    /// Width of the keys and of the values: one head_dim per KV head.
    pub fn kv_dim(&self) -> usize {
        self.n_head_kv * self.head_dim
    }

    /// Bytes of f16 KV cache for `tokens` tokens of context.
    pub fn kv_bytes(&self, tokens: usize) -> Option<usize> {
        self.n_layer
            .checked_mul(self.kv_dim())?
            .checked_mul(tokens)?
            .checked_mul(2 * core::mem::size_of::<u16>())
    }
}

/// The qwen3 keys the engine reads or checks.
const QWEN3_KEYS: [&str; 15] = [
    "qwen3.block_count",
    "qwen3.context_length",
    "qwen3.embedding_length",
    "qwen3.feed_forward_length",
    "qwen3.attention.head_count",
    "qwen3.attention.head_count_kv",
    "qwen3.attention.key_length",
    "qwen3.attention.value_length",
    "qwen3.attention.layer_norm_rms_epsilon",
    "qwen3.rope.freq_base",
    "qwen3.rope.dimension_count",
    "qwen3.rope.scaling.type",
    "qwen3.rope.scaling.factor",
    "qwen3.rope.scaling.attn_factor",
    "qwen3.rope.scale_linear",
];

/// Metadata the engine reads, checks or may ignore: descriptions (`general.*`),
/// the tokenizer, notes on how the file was quantised, and the qwen3 keys above.
/// Anything else, such as llama.cpp's `prism.hadamard.*` (weights stored rotated)
/// or `split.*` (a model in several files), could change what the model means.
fn known_key(key: &str) -> bool {
    ["general.", "tokenizer.", "quantize."]
        .iter()
        .any(|prefix| key.starts_with(prefix))
        || QWEN3_KEYS.contains(&key)
}

/// Refuses RoPE scaling in any form llama.cpp would apply: a scaling type other
/// than none, a scaling factor (or the older scale_linear) other than 0 or 1, or
/// an attention factor other than 1. The engine builds unscaled angles.
fn rope_unscaled(header: &Header<'_>) -> Result<(), Error> {
    let scaled = Err(Error::Unsupported("rope scaling"));
    if let Some(kind) = header.get("qwen3.rope.scaling.type")
        && kind.as_str() != Some("none")
    {
        return scaled;
    }
    for key in ["qwen3.rope.scaling.factor", "qwen3.rope.scale_linear"] {
        if let Some(value) = header.get(key)
            && !matches!(value.as_f32(), Some(f) if f == 0.0 || f == 1.0)
        {
            return scaled;
        }
    }
    if let Some(value) = header.get("qwen3.rope.scaling.attn_factor")
        && value.as_f32() != Some(1.0)
    {
        return scaled;
    }
    Ok(())
}
