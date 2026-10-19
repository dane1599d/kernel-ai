//! Every tensor a qwen3 model needs, with the type and shape the config implies.
//! [`check`] needs only the header, so `model pin` can refuse a file from its
//! first megabytes; [`Weights::bind`] then borrows the data. A file with a
//! missing, misshapen, mistyped or unknown tensor is refused, so nothing the
//! forward pass ignores (a bias, say) can change what the model means.

use crate::{Config, Error};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use gguf::{Gguf, Header, TensorType};

/// One tensor the model needs: its name, its type and its dims, innermost first.
struct Expected {
    name: String,
    ty: TensorType,
    dims: [usize; 2],
    rank: usize,
}

fn matrix(name: String, input: usize, output: usize) -> Expected {
    Expected {
        name,
        ty: TensorType::Q8_0,
        dims: [input, output],
        rank: 2,
    }
}

fn vector(name: String, len: usize) -> Expected {
    Expected {
        name,
        ty: TensorType::F32,
        dims: [len, 1],
        rank: 1,
    }
}

const LAYER_TENSORS: usize = 11;

/// Everything a file of this config must hold, in a fixed order: the embeddings,
/// the final norm, then 11 tensors per layer.
fn expected(config: &Config) -> Vec<Expected> {
    let (e, q, kv, ff, hd) = (
        config.n_embd,
        config.q_dim(),
        config.kv_dim(),
        config.n_ff,
        config.head_dim,
    );
    let mut list = Vec::with_capacity(2 + LAYER_TENSORS * config.n_layer);
    list.push(matrix("token_embd.weight".into(), e, config.vocab));
    list.push(vector("output_norm.weight".into(), e));
    for i in 0..config.n_layer {
        let name = |part: &str| format!("blk.{i}.{part}.weight");
        list.extend([
            vector(name("attn_norm"), e),
            matrix(name("attn_q"), e, q),
            matrix(name("attn_k"), e, kv),
            matrix(name("attn_v"), e, kv),
            vector(name("attn_q_norm"), hd),
            vector(name("attn_k_norm"), hd),
            matrix(name("attn_output"), q, e),
            vector(name("ffn_norm"), e),
            matrix(name("ffn_gate"), e, ff),
            matrix(name("ffn_up"), e, ff),
            matrix(name("ffn_down"), ff, e),
        ]);
    }
    list
}

/// The untied output projection, if the file has one.
const OUTPUT: &str = "output.weight";

/// Checks that the header holds exactly the tensors `config` needs, each of the
/// right type and shape. `output.weight` is optional (Qwen3-0.6B ties it to the
/// embeddings); any other tensor is refused.
pub fn check(header: &Header<'_>, config: &Config) -> Result<(), Error> {
    let mut wanted = expected(config);
    wanted.push(matrix(OUTPUT.into(), config.n_embd, config.vocab));
    for want in &wanted {
        let Some(info) = header.tensor(&want.name) else {
            if want.name == OUTPUT {
                continue;
            }
            return Err(Error::Tensor(shown(&want.name), "missing"));
        };
        if info.ty != want.ty {
            return Err(Error::Tensor(shown(&want.name), "has the wrong type"));
        }
        let dims = info.dims();
        let same = dims.len() == want.rank
            && dims
                .iter()
                .zip(&want.dims)
                .all(|(&have, &want)| usize::try_from(have) == Ok(want));
        if !same {
            return Err(Error::Tensor(shown(&want.name), "has the wrong shape"));
        }
    }
    let known = header.tensors().len();
    let has_output = header.tensor(OUTPUT).is_some() as usize;
    if known != wanted.len() - 1 + has_output {
        let extra = header
            .tensors()
            .iter()
            .find(|t| !wanted.iter().any(|w| w.name == t.name))
            .map_or_else(String::new, |t| shown(t.name));
        return Err(Error::Tensor(extra, "is not part of a qwen3 model"));
    }
    // The tokenizer names exactly one embedding row per token.
    let tokens = header
        .get("tokenizer.ggml.tokens")
        .and_then(|v| v.as_array())
        .map(|a| a.len());
    if tokens != Some(config.vocab) {
        return Err(Error::BadValue("tokenizer.ggml.tokens"));
    }
    // No two tensors share bytes, so what bind copies out is bounded by the file.
    let mut spans: Vec<(u64, u64)> = header
        .tensors()
        .iter()
        .map(|t| (t.offset, t.size))
        .collect();
    spans.sort_unstable();
    for pair in spans.windows(2) {
        let ((start, size), (next, _)) = (pair[0], pair[1]);
        if start.checked_add(size).is_none_or(|end| end > next) {
            return Err(Error::Overlap);
        }
    }
    Ok(())
}

/// A name from the file as an error may print it: at most 64 bytes, with any
/// control character escaped.
pub(crate) fn shown(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if out.len() >= 64 {
            out.push_str("...");
            break;
        }
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// One block's weights. Matrices stay as Q8_0 bytes in the file; norms are copied
/// out as f32, since the file gives them no alignment.
pub struct Layer<'a> {
    pub attn_norm: Vec<f32>,
    pub wq: &'a [u8],
    pub wk: &'a [u8],
    pub wv: &'a [u8],
    pub q_norm: Vec<f32>,
    pub k_norm: Vec<f32>,
    pub wo: &'a [u8],
    pub ffn_norm: Vec<f32>,
    pub w_gate: &'a [u8],
    pub w_up: &'a [u8],
    pub w_down: &'a [u8],
}

pub struct Weights<'a> {
    pub token_embd: &'a [u8],
    /// `output.weight`, or the embeddings when the file ties them.
    pub output: &'a [u8],
    pub output_norm: Vec<f32>,
    pub layers: Vec<Layer<'a>>,
}

impl<'a> Weights<'a> {
    /// Borrows every tensor of a file [`check`] accepted.
    pub fn bind(file: &Gguf<'a>, config: &Config) -> Result<Self, Error> {
        check(file.header(), config)?;
        let data = |name: &str| {
            file.tensor(name)
                .map(|t| t.data)
                .ok_or_else(|| Error::Tensor(name.into(), "missing"))
        };
        let floats = |name: &str| data(name).map(read_f32s);
        let token_embd = data("token_embd.weight")?;
        let output = match file.tensor(OUTPUT) {
            Some(t) => t.data,
            None => token_embd,
        };
        let mut layers = Vec::with_capacity(config.n_layer);
        for i in 0..config.n_layer {
            let name = |part: &str| format!("blk.{i}.{part}.weight");
            layers.push(Layer {
                attn_norm: floats(&name("attn_norm"))?,
                wq: data(&name("attn_q"))?,
                wk: data(&name("attn_k"))?,
                wv: data(&name("attn_v"))?,
                q_norm: floats(&name("attn_q_norm"))?,
                k_norm: floats(&name("attn_k_norm"))?,
                wo: data(&name("attn_output"))?,
                ffn_norm: floats(&name("ffn_norm"))?,
                w_gate: data(&name("ffn_gate"))?,
                w_up: data(&name("ffn_up"))?,
                w_down: data(&name("ffn_down"))?,
            });
        }
        Ok(Self {
            token_embd,
            output,
            output_norm: floats("output_norm.weight")?,
            layers,
        })
    }
}

fn read_f32s(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect()
}
