//! The qwen3 forward pass for one token: RMSNorm, Q8_0 projections, per-head
//! QK-norm, NeoX RoPE, grouped-query attention over the f32 KV cache, a SwiGLU
//! feed-forward block, and the output projection (tied to the embeddings in
//! Qwen3-0.6B). Norms and softmax sum in f64 and the RoPE angles are built as
//! llama.cpp builds them, so greedy answers follow llama.cpp's as closely as an
//! f32 cache allows.

use crate::{Config, Error, KvCache, Rope, Weights};
use alloc::vec;
use alloc::vec::Vec;
use gguf::Gguf;
use quant::{QVec, Tier, dequantize_row, matvec, row_bytes};

/// Buffers for one token, allocated once.
struct Scratch {
    x: Vec<f32>,
    xn: Vec<f32>,
    q: Vec<f32>,
    k: Vec<f32>,
    v: Vec<f32>,
    att: Vec<f32>,
    scores: Vec<f32>,
    gate: Vec<f32>,
    up: Vec<f32>,
    out: Vec<f32>,
    q_embd: QVec,
    q_att: QVec,
    q_ff: QVec,
}

pub struct Model<'a> {
    config: Config,
    weights: Weights<'a>,
    tier: Tier,
    rope: Rope,
    context: usize,
    s: Scratch,
}

impl<'a> Model<'a> {
    /// Checks the file and sets it up for up to `context` tokens of context.
    pub fn new(file: &Gguf<'a>, tier: Tier, context: usize) -> Result<Self, Error> {
        let config = Config::from_gguf(file.header())?;
        if context == 0 || context > config.context_length {
            return Err(Error::Context);
        }
        let weights = Weights::bind(file, &config)?;
        let rope = Rope::new(&config, context)?;
        let (e, q, kv, ff) = (config.n_embd, config.q_dim(), config.kv_dim(), config.n_ff);
        let s = Scratch {
            x: vec![0.0; e],
            xn: vec![0.0; e],
            q: vec![0.0; q],
            k: vec![0.0; kv],
            v: vec![0.0; kv],
            att: vec![0.0; q],
            scores: vec![0.0; context],
            gate: vec![0.0; ff],
            up: vec![0.0; ff],
            out: vec![0.0; e],
            q_embd: QVec::new(e),
            q_att: QVec::new(q),
            q_ff: QVec::new(ff),
        };
        Ok(Self {
            config,
            weights,
            tier,
            rope,
            context,
            s,
        })
    }

    /// A KV cache for this model's whole context cap.
    pub fn kv_cache(&self) -> Result<KvCache, Error> {
        KvCache::new(&self.config, self.context)
    }

    /// The shape the model was checked and sized for.
    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn tier(&self) -> Tier {
        self.tier
    }

    pub fn context(&self) -> usize {
        self.context
    }

    /// Runs `token` at the next position of `kv`, filling `logits` (one per
    /// vocabulary entry) when it is given; prompt tokens before the last skip the
    /// output projection, the largest matrix. Every check comes before the cache
    /// changes, so an error leaves it as it was.
    pub fn forward(
        &mut self,
        kv: &mut KvCache,
        token: u32,
        logits: Option<&mut [f32]>,
    ) -> Result<(), Error> {
        let c = self.config;
        if token as usize >= c.vocab {
            return Err(Error::Token(token));
        }
        if logits.as_deref().is_some_and(|l| l.len() != c.vocab) {
            return Err(Error::Logits);
        }
        if !self.fits(kv) {
            return Err(Error::CacheShape);
        }
        let pos = kv.len();
        if pos >= kv.capacity() {
            return Err(Error::ContextFull);
        }
        let row = row_bytes(c.n_embd);
        let start = token as usize * row;
        dequantize_row(&self.weights.token_embd[start..start + row], &mut self.s.x);
        for layer in 0..c.n_layer {
            self.attention(kv, layer, pos);
            self.feed_forward(layer);
        }
        kv.advance();
        if let Some(logits) = logits {
            let s = &mut self.s;
            rms_norm(&s.x, &self.weights.output_norm, c.rms_eps, &mut s.xn);
            s.q_embd.quantize(&s.xn);
            matvec(self.tier, self.weights.output, &s.q_embd, logits);
        }
        Ok(())
    }

    /// Whether `kv` was made for this model: its shape, and no more positions
    /// than the RoPE table covers.
    pub fn fits(&self, kv: &KvCache) -> bool {
        kv.fits(&self.config) && kv.capacity() <= self.context
    }

    fn attention(&mut self, kv: &mut KvCache, layer: usize, pos: usize) {
        let c = self.config;
        let (hd, kv_dim) = (c.head_dim, c.kv_dim());
        let w = &self.weights.layers[layer];
        let s = &mut self.s;
        rms_norm(&s.x, &w.attn_norm, c.rms_eps, &mut s.xn);
        s.q_embd.quantize(&s.xn);
        matvec(self.tier, w.wq, &s.q_embd, &mut s.q);
        matvec(self.tier, w.wk, &s.q_embd, &mut s.k);
        matvec(self.tier, w.wv, &s.q_embd, &mut s.v);
        for head in s.q.chunks_exact_mut(hd) {
            rms_norm_in_place(head, &w.q_norm, c.rms_eps);
            self.rope.apply(head, pos);
        }
        for head in s.k.chunks_exact_mut(hd) {
            rms_norm_in_place(head, &w.k_norm, c.rms_eps);
            self.rope.apply(head, pos);
        }
        let (k_slot, v_slot) = kv.slot_mut(layer, pos);
        k_slot.copy_from_slice(&s.k);
        v_slot.copy_from_slice(&s.v);
        let (keys, values) = kv.layer(layer, pos);
        let group = c.n_head / c.n_head_kv;
        let scale = 1.0 / libm::sqrtf(hd as f32);
        let scores = &mut s.scores[..=pos];
        for (h, (q, out)) in
            s.q.chunks_exact(hd)
                .zip(s.att.chunks_exact_mut(hd))
                .enumerate()
        {
            let at = h / group * hd;
            for (t, score) in scores.iter_mut().enumerate() {
                *score = dot(q, &keys[t * kv_dim + at..][..hd]) * scale;
            }
            softmax(scores);
            out.fill(0.0);
            for (t, &p) in scores.iter().enumerate() {
                for (o, &v) in out.iter_mut().zip(&values[t * kv_dim + at..][..hd]) {
                    *o += p * v;
                }
            }
        }
        s.q_att.quantize(&s.att);
        matvec(self.tier, w.wo, &s.q_att, &mut s.out);
        for (x, o) in s.x.iter_mut().zip(&s.out) {
            *x += o;
        }
    }

    fn feed_forward(&mut self, layer: usize) {
        let c = self.config;
        let w = &self.weights.layers[layer];
        let s = &mut self.s;
        rms_norm(&s.x, &w.ffn_norm, c.rms_eps, &mut s.xn);
        s.q_embd.quantize(&s.xn);
        matvec(self.tier, w.w_gate, &s.q_embd, &mut s.gate);
        matvec(self.tier, w.w_up, &s.q_embd, &mut s.up);
        for (g, u) in s.gate.iter_mut().zip(&s.up) {
            *g = *g / (1.0 + libm::expf(-*g)) * u;
        }
        s.q_ff.quantize(&s.gate);
        matvec(self.tier, w.w_down, &s.q_ff, &mut s.out);
        for (x, o) in s.x.iter_mut().zip(&s.out) {
            *x += o;
        }
    }
}

/// `x / rms(x) * weight`; the sum of squares runs in f64, as in llama.cpp.
fn rms_norm(x: &[f32], weight: &[f32], eps: f32, out: &mut [f32]) {
    let scale = rms_scale(x, eps);
    for ((o, &v), &w) in out.iter_mut().zip(x).zip(weight) {
        *o = v * scale * w;
    }
}

fn rms_norm_in_place(x: &mut [f32], weight: &[f32], eps: f32) {
    let scale = rms_scale(x, eps);
    for (v, &w) in x.iter_mut().zip(weight) {
        *v = *v * scale * w;
    }
}

fn rms_scale(x: &[f32], eps: f32) -> f32 {
    let sum: f64 = x.iter().map(|&v| f64::from(v * v)).sum();
    let mean = (sum / x.len() as f64) as f32;
    1.0 / libm::sqrtf(mean + eps)
}

/// A dot product in eight running sums, so it vectorises while its order stays
/// fixed in the source.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut sums = [0.0f32; 8];
    let (a8, a_tail) = a.as_chunks::<8>();
    let (b8, b_tail) = b.as_chunks::<8>();
    for (a, b) in a8.iter().zip(b8) {
        for i in 0..8 {
            sums[i] += a[i] * b[i];
        }
    }
    for (i, (x, y)) in a_tail.iter().zip(b_tail).enumerate() {
        sums[i] += x * y;
    }
    ((sums[0] + sums[1]) + (sums[2] + sums[3])) + ((sums[4] + sums[5]) + (sums[6] + sums[7]))
}

/// Softmax in place: the largest score is subtracted first, and the sum runs in
/// f64 as in llama.cpp.
fn softmax(scores: &mut [f32]) {
    let max = scores.iter().fold(f32::NEG_INFINITY, |m, &v| m.max(v));
    let mut sum = 0.0f64;
    for v in scores.iter_mut() {
        *v = libm::expf(*v - max);
        sum += f64::from(*v);
    }
    let inv = (1.0 / sum) as f32;
    for v in scores.iter_mut() {
        *v *= inv;
    }
}
