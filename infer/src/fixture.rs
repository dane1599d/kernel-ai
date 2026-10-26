//! Generated qwen3 models for tests and benchmarks, so CI never needs the real
//! file. The vocabulary is GPT-2's 256 byte tokens, a few merges and the chat
//! specials, padded with unused tokens; the weights are random Q8_0 blocks, or a
//! Q4_K_M file's mix of Q4_K and Q6_K ([`Quant::KMix`]), scaled so activations
//! stay near unit size. A spec always gives the same bytes.

mod vocab;

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use gguf::TensorType;
use gguf::write::Writer;

/// The matrices' formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quant {
    /// Every matrix Q8_0, as Qwen3-0.6B's file has them.
    Q8_0,
    /// A Q4_K_M file's mix: Q6_K embeddings (and output), Q6_K attention values
    /// and feed-forward down projections in even layers, Q4_K everywhere else.
    /// Every matrix input must be a multiple of 256.
    KMix,
}

impl Quant {
    /// The type of matrix `part` of `layer`, or of a whole-model matrix with no
    /// layer.
    fn of(self, part: &str, layer: Option<usize>) -> TensorType {
        match (self, layer) {
            (Self::Q8_0, _) => TensorType::Q8_0,
            (Self::KMix, None) => TensorType::Q6_K,
            (Self::KMix, Some(i)) if i % 2 == 0 && (part == "attn_v" || part == "ffn_down") => {
                TensorType::Q6_K
            }
            (Self::KMix, Some(_)) => TensorType::Q4_K,
        }
    }
}

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
    /// Zero the `output.weight` rows of every token that does not stand for plain
    /// text (ASCII letters, digits and spaces), so their logits are exactly 0 and
    /// greedy decoding picks plain text (needs `untied`).
    pub printable: bool,
    /// With `printable`, keep the `"` token's output row too, four times as
    /// strong, so under a JSON shape the model ends its strings within a few
    /// words.
    pub closes_strings: bool,
    pub quant: Quant,
    /// `general.name`.
    pub name: &'static str,
    /// `general.license`, if any.
    pub license: Option<&'static str>,
    pub seed: u64,
}

/// The name of the model the `local-ask` boot case answers from.
pub const LOCAL_ASK_NAME: &str = "kai test model";
/// Bytes after the local-ask model's last tensor. GGUF readers ignore what follows
/// the tensor data; these make the file an odd number of bytes, not whole sectors,
/// so a guest reading it must handle a partial last sector.
const LOCAL_ASK_TAIL: &[u8] = b"\nKernel AI local-ask fixture: bytes after the last tensor.\n";

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
            printable: false,
            closes_strings: false,
            quant: Quant::Q8_0,
            name: "generated fixture",
            license: None,
            seed: 1,
        }
    }

    /// Qwen3-4B's shapes in its Q4_K_M file's mix, with `n_layer` of its 36
    /// layers and random weights, for timing the engine without the real file:
    /// about 57 MB a layer and 320 MB of embeddings.
    pub fn qwen3_4b_layers(n_layer: usize) -> Self {
        Self {
            n_layer,
            n_embd: 2560,
            n_ff: 9728,
            n_head: 32,
            n_head_kv: 8,
            head_dim: 128,
            vocab: 151_936,
            context_length: 40_960,
            quant: Quant::KMix,
            ..Self::tiny()
        }
    }

    /// tiny's layout at the narrowest widths K-quants take (rows of 256 values),
    /// in a Q4_K_M file's mix of Q4_K and Q6_K: about 750 KB.
    pub fn tiny_k() -> Self {
        Self {
            n_embd: 256,
            n_ff: 512,
            head_dim: 64,
            quant: Quant::KMix,
            seed: 21,
            ..Self::tiny()
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
            ..Self::tiny()
        }
    }

    /// The model the `local-ask` boot case answers from: one layer of
    /// [`Spec::tiny_k`]'s shapes in its Q4_K and Q6_K mix (L6), so CI runs the
    /// K-quant path, an output that reaches only plain-text tokens, and a trained
    /// context of 152 positions. `ask`'s prompt takes 130 of them, so the answer
    /// stops at the context's end after 23 tokens, in seconds even when QEMU
    /// emulates the CPU.
    pub fn local_ask() -> Self {
        Self {
            context_length: 152,
            untied: true,
            printable: true,
            name: LOCAL_ASK_NAME,
            license: Some("MPL-2.0"),
            seed: 10,
            n_layer: 1,
            n_ff: 256,
            ..Self::tiny_k()
        }
    }

    /// The model the `local-script` and `live-local` boot cases ask: local-ask's
    /// shapes, the llm service's whole context cap, an output that reaches only
    /// words and the quote, which ends its strings, and room in the vocabulary
    /// for merges trained on the system prompts ([`local_script`]).
    pub fn local_script() -> Self {
        Self {
            context_length: 2048,
            vocab: 3072,
            closes_strings: true,
            // Seed 11 closed its script at once under jump-forward, so the
            // local-script case no longer reached the shell's checks; 12 did,
            // until the pack's script call lost its code. 16 called `get_policy`
            // until the process and service rows joined the pack, then
            // `threads()`, whose counters differ in the guest; 31 calls
            // `get_policy`, whose answer the guest gives too.
            seed: 31,
            ..Self::local_ask()
        }
    }
}

/// The local-script model file: [`Spec::local_script`], with a vocabulary of the
/// bytes, the chat specials and byte-pair merges learned from `text` (the system
/// prompts the llm service sends), filled up with filler words. Its prompts then
/// take a few hundred tokens rather than one per byte.
pub fn local_script(text: &str) -> Vec<u8> {
    let spec = Spec::local_script();
    let mut tokens: Vec<String> = (0..=255u8).map(|b| bpe::bytes::to_char(b).into()).collect();
    let mut types = alloc::vec![1; tokens.len()];
    for (text, ty) in SPECIALS {
        tokens.push(text.into());
        types.push(ty);
    }
    let (merges, made) = vocab::train(text, spec.vocab - tokens.len());
    for token in made {
        assert!(
            !SPECIALS.iter().any(|(name, _)| *name == token),
            "a merge spells {token}"
        );
        tokens.push(token);
        types.push(1);
    }
    let taken: BTreeSet<String> = tokens.iter().cloned().collect();
    let fillers = (0..)
        .map(|i| format!("\u{120}w{i}"))
        .filter(|name| !taken.contains(name));
    let room = spec.vocab - tokens.len();
    tokens.extend(fillers.take(room));
    types.resize(tokens.len(), 1);
    build_with(&spec, &tokens, &types, &merges)
}

/// The local-ask model file: [`Spec::local_ask`] and a short tail that makes its
/// length odd.
pub fn local_ask() -> Vec<u8> {
    let mut bytes = build(&Spec::local_ask());
    bytes.extend_from_slice(LOCAL_ASK_TAIL);
    if bytes.len().is_multiple_of(2) {
        bytes.push(b'\n');
    }
    bytes
}

/// Whether a token of [`vocabulary`] stands for plain text: a normal token whose
/// bytes are all ASCII letters, digits and spaces.
fn printable_token(text: &str, ty: i32) -> bool {
    ty == 1
        && text.chars().all(|c| {
            bpe::bytes::from_char(c).is_some_and(|b| b == b' ' || b.is_ascii_alphanumeric())
        })
}

/// Multiplies a Q8_0 row's block scales by `factor`, which multiplies its dot
/// products, and so its token's logit, by about as much.
fn strengthen(row: &mut [u8], factor: f32) {
    for block in row.chunks_mut(quant::BLOCK_BYTES) {
        let scale = quant::f16::to_f32(u16::from_le_bytes([block[0], block[1]]));
        block[..2].copy_from_slice(&quant::f16::from_f32(scale * factor).to_le_bytes());
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

/// Quantises a row of f32 into its blocks.
type Quantizer = fn(&[f32], &mut [u8]);

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

    /// A matrix of `ty` with each row `r` scaled by `scale(r)`, 0 for a row of
    /// zeros. Q8_0 rows are [`Rng::matrix`]'s, their block scales multiplied; a
    /// K-quant row is uniform values spread so its dot product with a unit-size
    /// input is about unit size, quantised by quant's plain scheme.
    fn matrix_of(
        &mut self,
        ty: TensorType,
        cols: usize,
        rows: usize,
        scale: impl Fn(usize) -> f32,
    ) -> Vec<u8> {
        if ty == TensorType::Q8_0 {
            let mut out = self.matrix(cols, rows);
            for (r, row) in out.chunks_mut(quant::row_bytes(cols)).enumerate() {
                let factor = scale(r);
                if factor == 0.0 {
                    row.fill(0);
                } else if factor != 1.0 {
                    strengthen(row, factor);
                }
            }
            return out;
        }
        let (bytes, quantize): (usize, Quantizer) = match ty {
            TensorType::Q4_K => (quant::q4_k::row_bytes(cols), quant::q4_k::quantize_row),
            _ => (quant::q6_k::row_bytes(cols), quant::q6_k::quantize_row),
        };
        let spread = libm::sqrtf(3.0 / cols as f32);
        let mut out = alloc::vec![0u8; bytes * rows];
        let mut values = alloc::vec![0.0f32; cols];
        for (r, row) in out.chunks_mut(bytes).enumerate() {
            let factor = scale(r) * spread;
            for v in values.iter_mut() {
                *v = (2.0 * self.unit() - 1.0) * factor;
            }
            quantize(&values, row);
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
    build_with(spec, &tokens, &types, &MERGES)
}

/// The GGUF file for `spec` with this vocabulary.
fn build_with(
    spec: &Spec,
    tokens: &[String],
    types: &[i32],
    merges: &[impl AsRef<str>],
) -> Vec<u8> {
    let eos = tokens.iter().position(|t| t == "<|im_end|>").unwrap_or(0);
    let mut w = Writer::new();
    let u32_of = |n: usize| n as u32;
    w.string("general.architecture", "qwen3")
        .string("general.name", spec.name);
    if let Some(license) = spec.license {
        w.string("general.license", license);
    }
    w.u32("qwen3.block_count", u32_of(spec.n_layer))
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
        .strings("tokenizer.ggml.tokens", tokens)
        .i32s("tokenizer.ggml.token_type", types)
        .strings("tokenizer.ggml.merges", merges)
        .u32("tokenizer.ggml.eos_token_id", u32_of(eos))
        .bool("tokenizer.ggml.add_bos_token", false);
    let mut rng = Rng(spec.seed);
    let (e, ff, hd) = (spec.n_embd, spec.n_ff, spec.head_dim);
    let (q, kv) = (spec.n_head * hd, spec.n_head_kv * hd);
    let embeddings = [e as u64, spec.vocab as u64];
    let whole = spec.quant.of("output", None);
    let embd = rng.matrix_of(whole, e, spec.vocab, |_| 1.0);
    w.tensor("token_embd.weight", &embeddings, whole, &embd);
    if spec.untied {
        let scale = |r: usize| match (&tokens[r], types[r]) {
            _ if !spec.printable => 1.0,
            (text, _) if spec.closes_strings && text == "\"" => 4.0,
            (text, ty) if printable_token(text, ty) => 1.0,
            _ => 0.0,
        };
        let output = rng.matrix_of(whole, e, spec.vocab, scale);
        w.tensor("output.weight", &embeddings, whole, &output);
    }
    let mut matrix = |w: &mut Writer, layer: usize, part: &str, cols: usize, rows: usize| {
        let ty = spec.quant.of(part, Some(layer));
        let data = rng.matrix_of(ty, cols, rows, |_| 1.0);
        let name = format!("blk.{layer}.{part}.weight");
        w.tensor(&name, &[cols as u64, rows as u64], ty, &data);
    };
    let mut norms = Rng(spec.seed ^ 0x5555);
    let mut norm = |w: &mut Writer, name: &str, len: usize| {
        w.tensor(name, &[len as u64], TensorType::F32, &norms.norm(len));
    };
    norm(&mut w, "output_norm.weight", e);
    for i in 0..spec.n_layer {
        let name = |part: &str| format!("blk.{i}.{part}.weight");
        norm(&mut w, &name("attn_norm"), e);
        matrix(&mut w, i, "attn_q", e, q);
        matrix(&mut w, i, "attn_k", e, kv);
        matrix(&mut w, i, "attn_v", e, kv);
        norm(&mut w, &name("attn_q_norm"), hd);
        norm(&mut w, &name("attn_k_norm"), hd);
        matrix(&mut w, i, "attn_output", q, e);
        norm(&mut w, &name("ffn_norm"), e);
        matrix(&mut w, i, "ffn_gate", e, ff);
        matrix(&mut w, i, "ffn_up", e, ff);
        matrix(&mut w, i, "ffn_down", ff, e);
    }
    w.finish()
}
