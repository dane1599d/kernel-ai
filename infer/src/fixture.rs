//! Generated qwen3 models for tests and benchmarks, so CI never needs the real
//! file. The vocabulary is GPT-2's 256 byte tokens, a few merges and the chat
//! specials, padded with unused tokens; the weights are random Q8_0 blocks scaled
//! so activations stay near unit size. A spec always gives the same bytes.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use gguf::TensorType;
use gguf::write::Writer;

#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub n_layer: usize,
    pub n_embd: usize,
    pub n_ff: usize,
    pub n_head: usize,
    pub n_head_kv: usize,
    pub head_dim: usize,
    pub vocab: usize,
    pub context_length: usize,
    /// Write a separate `output.weight` instead of tying it to the embeddings.
    pub untied: bool,
    pub seed: u64,
}

impl Spec {
    /// A model of about 130 KB that answers in milliseconds. Queries are wider
    /// than the residual stream, as in Qwen3-0.6B.
    pub fn tiny() -> Self {
        Self {
            n_layer: 2,
            n_embd: 64,
            n_ff: 96,
            n_head: 4,
            n_head_kv: 2,
            head_dim: 32,
            vocab: 384,
            context_length: 512,
            untied: false,
            seed: 1,
        }
    }

    /// Qwen3-0.6B's shapes, 640 MB of random weights, for measuring speed.
    pub fn qwen3_0_6b() -> Self {
        Self {
            n_layer: 28,
            n_embd: 1024,
            n_ff: 3072,
            n_head: 16,
            n_head_kv: 8,
            head_dim: 128,
            vocab: 151_936,
            context_length: 40_960,
            untied: false,
            seed: 1,
        }
    }
}

/// Merges for common English pairs, so encoding text exercises them.
const MERGES: [&str; 10] = [
    "\u{120} t",
    "h e",
    "\u{120}t he",
    "i n",
    "e r",
    "a n",
    "\u{120} a",
    "o n",
    "r e",
    "\u{120} s",
];

const SPECIALS: [(&str, i32); 5] = [
    ("<|endoftext|>", 3),
    ("<|im_start|>", 3),
    ("<|im_end|>", 3),
    ("<think>", 4),
    ("</think>", 4),
];

/// The vocabulary: byte tokens, merge results, specials, then filler words
/// (" w300" and so on), so whatever a random model says prints as text.
pub fn vocabulary(size: usize) -> (Vec<String>, Vec<i32>) {
    let mut tokens: Vec<String> = (0..=255u8).map(|b| bpe::bytes::to_char(b).into()).collect();
    tokens.extend(MERGES.iter().map(|m| m.replace(' ', "")));
    let mut types = alloc::vec![1; tokens.len()];
    for (text, ty) in SPECIALS {
        tokens.push(text.into());
        types.push(ty);
    }
    assert!(
        size >= tokens.len(),
        "a fixture vocabulary of {size} tokens"
    );
    for i in tokens.len()..size {
        tokens.push(format!("\u{120}w{i}"));
        types.push(1);
    }
    (tokens, types)
}

/// splitmix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// A Q8_0 matrix of `rows` rows with `cols` inputs each. Random bytes, -128
    /// included, have a spread of about 74, so each block's scale makes a row's
    /// dot product with a unit-size input about unit size.
    fn matrix(&mut self, cols: usize, rows: usize) -> Vec<u8> {
        let mut out = alloc::vec![0u8; quant::row_bytes(cols) * rows];
        let base = 1.0 / (74.0 * libm::sqrtf(cols as f32));
        for block in out.as_chunks_mut::<{ quant::BLOCK_BYTES }>().0 {
            let scale = base * (0.5 + self.unit());
            block[..2].copy_from_slice(&quant::f16::from_f32(scale).to_le_bytes());
            for chunk in block[2..].chunks_mut(8) {
                let bits = self.next().to_le_bytes();
                chunk.copy_from_slice(&bits[..chunk.len()]);
            }
        }
        out
    }

    /// A norm's weights, near 1.
    fn norm(&mut self, len: usize) -> Vec<u8> {
        (0..len)
            .flat_map(|_| (0.75 + 0.5 * self.unit()).to_le_bytes())
            .collect()
    }
}

/// The GGUF file for `spec`.
pub fn build(spec: &Spec) -> Vec<u8> {
    let (tokens, types) = vocabulary(spec.vocab);
    let eos = tokens.iter().position(|t| t == "<|im_end|>").unwrap_or(0);
    let mut w = Writer::new();
    let u32_of = |n: usize| n as u32;
    w.string("general.architecture", "qwen3")
        .string("general.name", "generated fixture")
        .u32("qwen3.block_count", u32_of(spec.n_layer))
        .u32("qwen3.context_length", u32_of(spec.context_length))
        .u32("qwen3.embedding_length", u32_of(spec.n_embd))
        .u32("qwen3.feed_forward_length", u32_of(spec.n_ff))
        .u32("qwen3.attention.head_count", u32_of(spec.n_head))
        .u32("qwen3.attention.head_count_kv", u32_of(spec.n_head_kv))
        .f32("qwen3.rope.freq_base", 1_000_000.0)
        .f32("qwen3.attention.layer_norm_rms_epsilon", 1e-6)
        .u32("qwen3.attention.key_length", u32_of(spec.head_dim))
        .u32("qwen3.attention.value_length", u32_of(spec.head_dim))
        .string("tokenizer.ggml.model", "gpt2")
        .string("tokenizer.ggml.pre", "qwen2")
        .strings("tokenizer.ggml.tokens", &tokens)
        .i32s("tokenizer.ggml.token_type", &types)
        .strings("tokenizer.ggml.merges", &MERGES)
        .u32("tokenizer.ggml.eos_token_id", u32_of(eos))
        .bool("tokenizer.ggml.add_bos_token", false);
    let mut rng = Rng(spec.seed);
    let (e, ff, hd) = (spec.n_embd, spec.n_ff, spec.head_dim);
    let (q, kv) = (spec.n_head * hd, spec.n_head_kv * hd);
    let mut matrix = |w: &mut Writer, name: &str, cols: usize, rows: usize| {
        let data = rng.matrix(cols, rows);
        w.tensor(name, &[cols as u64, rows as u64], TensorType::Q8_0, &data);
    };
    matrix(&mut w, "token_embd.weight", e, spec.vocab);
    if spec.untied {
        matrix(&mut w, "output.weight", e, spec.vocab);
    }
    let mut norms = Rng(spec.seed ^ 0x5555);
    let mut norm = |w: &mut Writer, name: &str, len: usize| {
        w.tensor(name, &[len as u64], TensorType::F32, &norms.norm(len));
    };
    norm(&mut w, "output_norm.weight", e);
    for i in 0..spec.n_layer {
        let name = |part: &str| format!("blk.{i}.{part}.weight");
        norm(&mut w, &name("attn_norm"), e);
        matrix(&mut w, &name("attn_q"), e, q);
        matrix(&mut w, &name("attn_k"), e, kv);
        matrix(&mut w, &name("attn_v"), e, kv);
        norm(&mut w, &name("attn_q_norm"), hd);
        norm(&mut w, &name("attn_k_norm"), hd);
        matrix(&mut w, &name("attn_output"), q, e);
        norm(&mut w, &name("ffn_norm"), e);
        matrix(&mut w, &name("ffn_gate"), e, ff);
        matrix(&mut w, &name("ffn_up"), e, ff);
        matrix(&mut w, &name("ffn_down"), ff, e);
    }
    w.finish()
}
